//! DNS stub resolver (RFC 1035). Only A records over UDP.

use core::fmt;

use alloc::vec::Vec;

use crate::{Be, Ipv4Addr, NetworkHeader};

const DNS_PORT: u16 = 53;

const MAX_LABEL: usize = 63;
const MAX_NAME_WIRE: usize = 255;

/// Header + longest encoded name (255) + QTYPE + QCLASS.
const MAX_QUERY: usize = DnsHeader::SIZE + MAX_NAME_WIRE + 4;

const FLAG_QR: u16 = 0x8000;
const FLAG_TC: u16 = 0x0200;
const FLAG_RD: u16 = 0x0100;

const OPCODE_MASK: u16 = 0x7800;
const RCODE_MASK: u16 = 0x000F;

const RCODE_NO_ERROR: u16 = 0;
const RCODE_NAME_ERROR: u16 = 3;

const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsError {
    /// The name can't be encoded: empty label, label over 63 bytes, or too long overall.
    BadName,
    /// The server says the name doesn't exist, or it has no A records.
    NotFound,
    /// The server answered with an error code, or the reply was truncated.
    ServerFailure,
}

impl fmt::Display for DnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DnsError::BadName => write!(f, "bad name"),
            DnsError::NotFound => write!(f, "name not found"),
            DnsError::ServerFailure => write!(f, "truncated or error response"),
        }
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
