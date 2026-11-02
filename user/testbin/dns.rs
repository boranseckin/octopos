#![no_std]
#![no_main]

use user::dns::{self, DnsError};
use user::*;

const ID: u16 = 0x1a2b;
const ADDR: Ipv4Addr = Ipv4Addr([192, 0, 2, 10]);
const ADDR2: Ipv4Addr = Ipv4Addr([192, 0, 2, 11]);

/// Large enough for any query.
const BUF_SIZE: usize = 512;

/// The A/IN query for "example.com" with id `ID` (RFC 1035 4.1).
const EXAMPLE_QUERY: [u8; 29] = [
    0x1a, 0x2b, // id
    0x01, 0x00, // flags: RD
    0x00, 0x01, // qdcount
    0x00, 0x00, // ancount
    0x00, 0x00, // nscount
    0x00, 0x00, // arcount
    7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', //
    3, b'c', b'o', b'm', //
    0,    // root
    0x00, 0x01, // qtype A
    0x00, 0x01, // qclass IN
];

/// Offset of the first answer record in a reply built by `reply`.
const FIRST_ANSWER: usize = EXAMPLE_QUERY.len();

const FLAGS_OK: u16 = 0x8180; // QR, RD, RA
const FLAG_TC: u16 = 0x0200;

const TYPE_A: u16 = 1;
const TYPE_CNAME: u16 = 5;
const TYPE_AAAA: u16 = 28;

/// Builds a reply to `EXAMPLE_QUERY`: the header with `flags` and one answer per entry, the
/// question echoed, then each answer named `c0 0c` (a pointer to the question's name).
fn reply(id: u16, flags: u16, answers: &[(u16, &[u8])]) -> Vec<u8> {
    let mut msg = Vec::from(EXAMPLE_QUERY);

    msg[0..2].copy_from_slice(&id.to_be_bytes());
    msg[2..4].copy_from_slice(&flags.to_be_bytes());
    msg[6..8].copy_from_slice(&(answers.len() as u16).to_be_bytes());

    for &(typ, rdata) in answers {
        msg.extend_from_slice(&[0xc0, 0x0c]); // name: pointer to offset 12
        msg.extend_from_slice(&typ.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes()); // class IN
        msg.extend_from_slice(&3600u32.to_be_bytes()); // ttl
        msg.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        msg.extend_from_slice(rdata);
    }

    msg
}

fn a(addr: &Ipv4Addr) -> (u16, &[u8]) {
    (TYPE_A, &addr.0)
}

fn encode(name: &str) -> Result<Vec<u8>, DnsError> {
    let mut buf = [0u8; BUF_SIZE];
    let n = dns::encode_query(&mut buf, ID, name)?;
    Ok(Vec::from(&buf[..n]))
}

/// "example.com" encodes to the exact bytes from RFC 1035.
fn test_encode_example() {
    assert_eq!(
        encode("example.com").as_deref(),
        Ok(&EXAMPLE_QUERY[..]),
        "query bytes"
    );
}

/// A single trailing dot names the same domain.
fn test_encode_trailing_dot() {
    assert_eq!(
        encode("example.com.").as_deref(),
        Ok(&EXAMPLE_QUERY[..]),
        "trailing dot must not change the query"
    );
}

/// The longest legal name, 255 bytes on the wire, is accepted.
fn test_encode_longest_name() {
    let label = "x".repeat(63);
    let name = format!("{label}.{label}.{label}.{}", "x".repeat(61));

    let query = encode(&name).expect("255-byte name must encode");
    // header + name + qtype + qclass
    assert_eq!(query.len(), 12 + 255 + 4, "query length");
}

/// Names that can't be encoded are rejected.
fn test_encode_rejects() {
    let long_label = format!("{}.com", "x".repeat(64));
    let label = "x".repeat(63);
    // one byte over the 255-byte limit on the wire
    let long_name = format!("{label}.{label}.{label}.{}", "x".repeat(62));

    let cases = [
        ("empty", ""),
        ("root only", "."),
        ("empty label", "a..b"),
        ("leading dot", ".example.com"),
        ("two trailing dots", "example.com.."),
        ("64-byte label", long_label.as_str()),
        ("256-byte name", long_name.as_str()),
    ];

    for (what, name) in cases {
        assert_eq!(
            encode(name),
            Err(DnsError::BadName),
            "{what} must be rejected"
        );
    }
}

/// A reply with one A record returns its address.
fn test_parse_a() {
    let msg = reply(ID, FLAGS_OK, &[a(&ADDR)]);
    assert_eq!(dns::parse_reply(&msg, ID), Some(Ok(vec![ADDR])));
}

/// A CNAME before the A record is skipped.
fn test_parse_cname_then_a() {
    let cname: &[u8] = &[3, b'w', b'w', b'w', 0xc0, 0x0c];
    let msg = reply(ID, FLAGS_OK, &[(TYPE_CNAME, cname), a(&ADDR)]);
    assert_eq!(dns::parse_reply(&msg, ID), Some(Ok(vec![ADDR])));
}

/// Every A record is returned, in order.
fn test_parse_multiple_a() {
    let msg = reply(ID, FLAGS_OK, &[a(&ADDR), a(&ADDR2)]);
    assert_eq!(dns::parse_reply(&msg, ID), Some(Ok(vec![ADDR, ADDR2])));
}

/// Records that aren't a 4-byte A/IN are skipped.
fn test_parse_skips_other_types() {
    let wide = [0u8; 16];
    let msg = reply(
        ID,
        FLAGS_OK,
        &[(TYPE_AAAA, &wide), (TYPE_A, &wide), a(&ADDR)],
    );
    assert_eq!(dns::parse_reply(&msg, ID), Some(Ok(vec![ADDR])));
}

/// "No such name" is NotFound.
fn test_parse_name_error() {
    let msg = reply(ID, FLAGS_OK | 3, &[]);
    assert_eq!(dns::parse_reply(&msg, ID), Some(Err(DnsError::NotFound)));
}

/// A name with no A records is NotFound.
fn test_parse_no_data() {
    let msg = reply(ID, FLAGS_OK, &[]);
    assert_eq!(dns::parse_reply(&msg, ID), Some(Err(DnsError::NotFound)));
}

/// Any other error code is ServerFailure.
fn test_parse_server_failure() {
    for rcode in [1, 2, 4, 5] {
        let msg = reply(ID, FLAGS_OK | rcode, &[]);
        assert_eq!(
            dns::parse_reply(&msg, ID),
            Some(Err(DnsError::ServerFailure)),
            "rcode {rcode}"
        );
    }
}

/// A truncated reply can't be retried over TCP, so it's a failure.
fn test_parse_truncated() {
    let msg = reply(ID, FLAGS_OK | FLAG_TC, &[a(&ADDR)]);
    assert_eq!(
        dns::parse_reply(&msg, ID),
        Some(Err(DnsError::ServerFailure))
    );
}

/// A reply to some other query is ignored.
fn test_parse_wrong_id() {
    let msg = reply(ID + 1, FLAGS_OK, &[a(&ADDR)]);
    assert_eq!(dns::parse_reply(&msg, ID), None);
}

/// A query is not a reply.
fn test_parse_not_response() {
    let msg = reply(ID, FLAGS_OK & !0x8000, &[a(&ADDR)]);
    assert_eq!(dns::parse_reply(&msg, ID), None);
}

/// Only standard queries are answered.
fn test_parse_wrong_opcode() {
    let msg = reply(ID, FLAGS_OK | (2 << 11), &[a(&ADDR)]);
    assert_eq!(dns::parse_reply(&msg, ID), None);
}

/// A reply must echo exactly one question.
fn test_parse_question_count() {
    for qdcount in [0u16, 2] {
        let mut msg = reply(ID, FLAGS_OK, &[a(&ADDR)]);
        msg[4..6].copy_from_slice(&qdcount.to_be_bytes());
        assert_eq!(dns::parse_reply(&msg, ID), None, "qdcount {qdcount}");
    }
}

/// Every truncation point is malformed, and none of them panics.
fn test_parse_every_prefix() {
    let msg = reply(ID, FLAGS_OK, &[a(&ADDR), a(&ADDR2)]);

    for n in 0..msg.len() {
        assert_eq!(dns::parse_reply(&msg[..n], ID), None, "prefix of {n} bytes");
    }
}

/// A label longer than what's left of the message is malformed.
fn test_parse_label_past_end() {
    let mut msg = reply(ID, FLAGS_OK, &[a(&ADDR)]);
    msg[FIRST_ANSWER] = 63; // a label that runs far past the end
    msg.truncate(FIRST_ANSWER + 2);
    assert_eq!(dns::parse_reply(&msg, ID), None);
}

/// A pointer to itself is skipped, not followed, so it can't loop.
fn test_parse_pointer_loop() {
    let mut msg = reply(ID, FLAGS_OK, &[a(&ADDR)]);
    msg[FIRST_ANSWER..FIRST_ANSWER + 2].copy_from_slice(&[0xc0, FIRST_ANSWER as u8]);
    assert_eq!(dns::parse_reply(&msg, ID), Some(Ok(vec![ADDR])));
}

/// The reserved label types (top bits 01 and 10) are malformed.
fn test_parse_reserved_label() {
    for byte in [0x40, 0x80] {
        let mut msg = reply(ID, FLAGS_OK, &[a(&ADDR)]);
        msg[FIRST_ANSWER] = byte;
        assert_eq!(dns::parse_reply(&msg, ID), None, "label byte {byte:#x}");
    }
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_encode_example();
    test_encode_trailing_dot();
    test_encode_longest_name();
    test_encode_rejects();

    test_parse_a();
    test_parse_cname_then_a();
    test_parse_multiple_a();
    test_parse_skips_other_types();

    test_parse_name_error();
    test_parse_no_data();
    test_parse_server_failure();
    test_parse_truncated();

    test_parse_wrong_id();
    test_parse_not_response();
    test_parse_wrong_opcode();
    test_parse_question_count();

    test_parse_every_prefix();
    test_parse_label_past_end();
    test_parse_pointer_loop();
    test_parse_reserved_label();
}
