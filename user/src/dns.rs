//! DNS stub resolver (RFC 1035). Only A records over UDP.

use core::fmt;
use core::str::FromStr;

use alloc::format;
use alloc::vec::Vec;

use crate::{
    Be, Fd, Ioctl, Ipv4Addr, NetworkHeader, OpenFlag, SockAddr, SocketType, SysError,
    TICKS_PER_SEC, close, ioctl, open, random, read, receive, send, socket, unlink, uptime, write,
};

const DNS_PORT: u16 = 53;

const MAX_LABEL: usize = 63;
const MAX_NAME_WIRE: usize = 255;

/// Header + longest encoded name (255) + QTYPE + QCLASS.
const MAX_QUERY: usize = DnsHeader::SIZE + MAX_NAME_WIRE + 4;
const MAX_MESSAGE: usize = 512;

const FLAG_QR: u16 = 0x8000;
const FLAG_TC: u16 = 0x0200;
const FLAG_RD: u16 = 0x0100;

const OPCODE_MASK: u16 = 0x7800;
const RCODE_MASK: u16 = 0x000F;

const RCODE_NO_ERROR: u16 = 0;
const RCODE_NAME_ERROR: u16 = 3;

const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;

const RESOLV_PATH: &str = "/etc/resolv";
pub const MAX_SERVERS: usize = 3;

const ATTEMPTS: usize = 3;
const ATTEMPT_TIMEOUT: usize = 2 * TICKS_PER_SEC;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsError {
    /// The name can't be encoded: empty label, label over 63 bytes, or too long overall.
    BadName,
    /// The server says the name doesn't exist, or it has no A records.
    NotFound,
    /// The server answered with an error code, or the reply was truncated.
    ServerFailure,
    /// `RESOLV_PATH` is missing or lists no servers.
    NoServers,
    /// No server answered in time.
    TimedOut,
    /// A syscall failed.
    Sys(SysError),
}

impl fmt::Display for DnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DnsError::BadName => write!(f, "bad name"),
            DnsError::NotFound => write!(f, "name not found"),
            DnsError::ServerFailure => write!(f, "truncated or error response"),
            DnsError::NoServers => write!(f, "no server found in {}", RESOLV_PATH),
            DnsError::TimedOut => write!(f, "query timed out"),
            DnsError::Sys(e) => write!(f, "{e}"),
        }
    }
}

impl From<SysError> for DnsError {
    fn from(value: SysError) -> Self {
        Self::Sys(value)
    }
}

/// DNS message header from RFC 1035 4.1.1
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct DnsHeader {
    id: Be<u16>,
    flags: Be<u16>,
    qdcount: Be<u16>,
    ancount: Be<u16>,
    nscount: Be<u16>,
    arcount: Be<u16>,
}

unsafe impl NetworkHeader for DnsHeader {}

impl DnsHeader {
    const SIZE: usize = size_of::<Self>();

    fn new_query(id: u16) -> Self {
        Self {
            id: Be::new(id),
            flags: Be::new(FLAG_RD),
            qdcount: Be::new(1),
            ancount: Be::new(0),
            nscount: Be::new(0),
            arcount: Be::new(0),
        }
    }

    fn rcode(&self) -> u16 {
        self.flags.get() & RCODE_MASK
    }
}

/// Bounds-checked reader over a received message.
/// Every method returns `None` past the end.
struct Reader<'a> {
    msg: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(msg: &'a [u8], pos: usize) -> Self {
        Self { msg, pos }
    }

    fn u8(&mut self) -> Option<u8> {
        let data = self.msg.get(self.pos).copied()?;
        self.pos += 1;
        Some(data)
    }

    fn u16(&mut self) -> Option<u16> {
        let data = u16::from_be_bytes(*self.msg.get(self.pos..)?.first_chunk()?);
        self.pos += 2;
        Some(data)
    }

    fn u32(&mut self) -> Option<u32> {
        let data = u32::from_be_bytes(*self.msg.get(self.pos..)?.first_chunk()?);
        self.pos += 4;
        Some(data)
    }

    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.saturating_add(n);
        let data = self.msg.get(self.pos..end)?;
        self.pos = end;
        Some(data)
    }

    /// Skips a name without following compression pointers (RFC 1035 4.1.4).
    fn skip_name(&mut self) -> Option<()> {
        loop {
            let len = self.u8()?;
            match len >> 6 {
                0b00 => {
                    // actual label
                    // if we see 0 byte, end of name.
                    if len == 0 {
                        return Some(());
                    } else {
                        self.pos += len as usize;
                    }
                }
                0b11 => {
                    // pointer to another part
                    // read one more byte (pointer), then stop
                    self.u8()?;
                    return Some(());
                }
                0b01 | 0b10 => return None,
                _ => unreachable!(),
            }
        }
    }
}

/// Writes `name` as length-prefixed labels, ending with a zero byte (RFC 1035 §3.1).
/// Returns the encoded length.
///
/// RFC 1035 4.1.2
/// A domain name represented as a sequence of labels where each label consists of a length octet
/// followed by that number of octets.
/// The domain name terminates with the zero length octet for the null label of the root.
/// Note that this field may be an odd number of octets; no padding is used.
fn encode_name(buf: &mut [u8], name: &str) -> Result<usize, DnsError> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() {
        return Err(DnsError::BadName);
    }

    let mut total = 0;

    for label in name.split('.') {
        let len = label.len();

        // label must be non-zero
        // label + length byte + zero byte should not exceed the limit
        if len == 0 || len > MAX_LABEL || total + 1 + len + 1 > MAX_NAME_WIRE {
            return Err(DnsError::BadName);
        }

        buf[total] = len as u8;
        total += 1;
        buf[total..total + len].copy_from_slice(label.as_bytes());
        total += len;
    }

    buf[total] = 0;
    total += 1;

    Ok(total)
}

/// Writes an A/IN query for `name` into `buf` and returns its length.
/// `buf` must be at least `MAX_QUERY` bytes.
pub fn encode_query(buf: &mut [u8], id: u16, name: &str) -> Result<usize, DnsError> {
    assert!(buf.len() >= MAX_QUERY, "buf smaller than MAX_QUERY");

    let mut offset = 0;
    DnsHeader::new_query(id).append_to(buf, &mut offset);
    offset += encode_name(&mut buf[offset..], name)?;
    buf[offset..offset + 2].copy_from_slice(&TYPE_A.to_be_bytes());
    offset += 2;
    buf[offset..offset + 2].copy_from_slice(&CLASS_IN.to_be_bytes());
    offset += 2;
    Ok(offset)
}

/// Parses a response to the query with `id`.
///
/// - `None`: not a valid response to this query.
/// - `Some(Err(_))`: the server answered with an error or a truncated reply.
/// - `Some(Ok(addrs))`: the A records in the answer section, never empty.
pub fn parse_reply(msg: &[u8], id: u16) -> Option<Result<Vec<Ipv4Addr>, DnsError>> {
    let (header, rest) = DnsHeader::from_bytes_with_rest(msg)?;

    let flags = header.flags.get();

    // id matches, QR = 1 (response), OPCODE = 0 (standard QUERY)
    if header.id.get() != id || flags & FLAG_QR == 0 || flags & OPCODE_MASK != 0 {
        return None;
    }

    // truncated message; we cannot handle it
    if flags & FLAG_TC != 0 {
        return Some(Err(DnsError::ServerFailure));
    }

    // server did not have a record
    if header.rcode() == RCODE_NAME_ERROR {
        return Some(Err(DnsError::NotFound));
    }

    // any other non-zero code is error
    if header.rcode() != RCODE_NO_ERROR {
        return Some(Err(DnsError::ServerFailure));
    }

    // expect only 1 question
    if header.qdcount.get() != 1 {
        return None;
    }

    let mut reader = Reader::new(rest, 0);

    // there is only one question, skip the name, qtype, and qclass
    reader.skip_name()?;
    let _qtype = reader.u16()?;
    let _qclass = reader.u16()?;

    let mut answers = Vec::new();

    for _ in 0..header.ancount.get() {
        reader.skip_name()?;
        let typ = reader.u16()?;
        let class = reader.u16()?;
        let _ttl = reader.u32()?;
        let rdlength = reader.u16()?;
        let rdata = reader.bytes(rdlength as usize)?;

        if typ == TYPE_A && class == CLASS_IN && rdlength == 4 {
            let rdata = rdata.first_chunk::<4>()?;
            answers.push(Ipv4Addr::from(*rdata));
        }
    }

    if answers.is_empty() {
        Some(Err(DnsError::NotFound))
    } else {
        Some(Ok(answers))
    }
}

/// Replaces `RESOLV_PATH` with `servers`, one per line.
/// An empty list removes the file.
pub fn set_servers(servers: &[Ipv4Addr]) -> Result<(), SysError> {
    if servers.is_empty() {
        return match unlink(RESOLV_PATH) {
            Ok(_) | Err(SysError::NoEntry) => Ok(()),
            Err(e) => Err(e),
        };
    }

    let fd = open(
        RESOLV_PATH,
        OpenFlag::CREATE | OpenFlag::WRITE_ONLY | OpenFlag::TRUNCATE,
    )?;

    for server in servers {
        if let Err(e) = write(fd, format!("{}\n", server).as_bytes()) {
            let _ = close(fd);
            return Err(e);
        }
    }

    close(fd)?;
    Ok(())
}

/// Reads up to `MAX_SERVERS` addresses from `RESOLV_PATH`, one per line.
fn get_servers() -> Result<Vec<Ipv4Addr>, DnsError> {
    let fd = open(RESOLV_PATH, OpenFlag::READ_ONLY).map_err(|_| DnsError::NoServers)?;

    let mut buf = [0u8; 256];
    let len = read(fd, &mut buf);

    let _ = close(fd);

    let len = len?;

    let servers = str::from_utf8(&buf[..len]).map_err(|_| DnsError::NoServers)?;
    let servers: Vec<_> = servers
        .lines()
        .flat_map(|l| Ipv4Addr::from_str(l.trim()))
        .take(MAX_SERVERS)
        .collect();

    if servers.is_empty() {
        return Err(DnsError::NoServers);
    }

    Ok(servers)
}

/// Sends one query to `server` and waits for its reply, retrying `ATTEMPTS` times.
pub fn query(server: SockAddr, name: &str) -> Result<Vec<Ipv4Addr>, DnsError> {
    query_with(server, name, ATTEMPT_TIMEOUT)
}

/// `query` with a custom per-attempt timeout, in ticks. Used for testing.
pub fn query_with(server: SockAddr, name: &str, timeout: usize) -> Result<Vec<Ipv4Addr>, DnsError> {
    let mut id_buf = [0u8; 2];
    random(&mut id_buf)?;
    let id = u16::from_ne_bytes(id_buf);

    let mut buf = [0u8; MAX_QUERY];
    let len = encode_query(&mut buf, id, name)?;

    let fd = socket(SocketType::Udp, SockAddr::any_inet())?;

    // attempt in a closure so that we can cleanup after
    let attempts = || -> Result<Vec<Ipv4Addr>, DnsError> {
        for _ in 0..ATTEMPTS {
            send(fd, &buf[..len], server)?;

            let deadline = uptime() + timeout;
            if let Some(reply) = wait_reply(fd, server, id, deadline)? {
                return reply;
            }
        }

        Err(DnsError::TimedOut)
    };

    let result = attempts();
    let _ = close(fd);
    result
}

/// Receives until a valid reply from `server` arrives or `deadline` passes.
/// `None` means this attempt timed out.
fn wait_reply(
    fd: Fd,
    server: SockAddr,
    id: u16,
    deadline: usize,
) -> Result<Option<Result<Vec<Ipv4Addr>, DnsError>>, SysError> {
    loop {
        let remaining = deadline.saturating_sub(uptime());
        if remaining == 0 {
            return Ok(None);
        }

        ioctl(fd, Ioctl::SOCKET_SET_RECV_TIMEOUT, remaining)?;

        let mut buf = [0u8; MAX_MESSAGE];
        let (len, source) = match receive(fd, &mut buf) {
            Ok(resp) => resp,
            Err(SysError::TimedOut) => return Ok(None),
            Err(e) => return Err(e),
        };

        if source != server {
            continue;
        }

        match parse_reply(&buf[..len], id) {
            Some(r) => return Ok(Some(r)),
            None => continue,
        }
    }
}

/// Resolves `name` using the servers in `RESOLV_PATH`, in order.
pub fn resolve(name: &str) -> Result<Vec<Ipv4Addr>, DnsError> {
    let mut last_err = None;

    for server in get_servers()? {
        match query(SockAddr::inet(server, DNS_PORT), name) {
            Ok(a) => return Ok(a),
            Err(e) => match e {
                // assume other servers would agree, so return early
                DnsError::NotFound | DnsError::BadName => {
                    return Err(e);
                }

                // save the error for now and check other servers
                DnsError::TimedOut | DnsError::ServerFailure | DnsError::Sys(_) => {
                    last_err = Some(e);
                }

                DnsError::NoServers => unreachable!(),
            },
        }
    }

    // no server resolved, return the latest error
    Err(last_err.expect("error since loop did not return"))
}

/// Returns `host` as an address if it is one; otherwise resolves it and returns the first address.
pub fn resolve_host(host: &str) -> Result<Ipv4Addr, DnsError> {
    if let Ok(addr) = Ipv4Addr::from_str(host) {
        return Ok(addr);
    }

    resolve(host)?.first().copied().ok_or(DnsError::NotFound)
}
