use proxy_core::packet::build_tcp_packet;
use proxy_core::packet::ParsedPacket;

#[test]
fn test_packet_roundtrip() {
    // Build a TCP SYN packet as the client would send it
    let syn = build_tcp_packet(
        [10, 0, 0, 2].into(),
        [104, 1, 219, 8].into(),
        49152,
        80,
        1000,   // client seq
        0,      // ack (not set in SYN)
        0x02,   // SYN
        &[],
    );
    assert!(!syn.is_empty());

    let parsed = ParsedPacket::parse(&syn).unwrap();
    assert!(parsed.is_tcp_syn());
    assert_eq!(parsed.ip.source.to_string(), "10.0.0.2");
    assert_eq!(parsed.ip.destination.to_string(), "104.1.219.8");
    assert_eq!(parsed.tcp.as_ref().unwrap().source_port, 49152);
    assert_eq!(parsed.tcp.as_ref().unwrap().destination_port, 80);
    assert!(parsed.payload.is_empty());

    // Simulate the Router building a SYN-ACK response
    let syn_ack = build_tcp_packet(
        [104, 1, 219, 8].into(),
        [10, 0, 0, 2].into(),
        80,
        49152,
        50000,  // server ISN
        1001,   // ack = client seq + 1
        0x12,   // SYN | ACK
        &[],
    );
    assert!(!syn_ack.is_empty());

    let resp_parsed = ParsedPacket::parse(&syn_ack).unwrap();
    assert_eq!(resp_parsed.ip.source.to_string(), "104.1.219.8");
    assert_eq!(resp_parsed.ip.destination.to_string(), "10.0.0.2");
    assert_eq!(resp_parsed.tcp.as_ref().unwrap().source_port, 80);
    assert_eq!(resp_parsed.tcp.as_ref().unwrap().destination_port, 49152);

    // Verify TCP flags
    let flags = &resp_parsed.tcp.as_ref().unwrap().flags;
    assert!(flags.syn);
    assert!(flags.ack);

    // Build a data-bearing packet and response
    let client_data = build_tcp_packet(
        [10, 0, 0, 2].into(),
        [104, 1, 219, 8].into(),
        49152,
        80,
        1001,   // seq = client_isn + 1
        50001,  // ack = server_isn + 1
        0x18,   // ACK | PSH
        b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n",
    );

    let parsed_data = ParsedPacket::parse(&client_data).unwrap();
    assert!(!parsed_data.is_tcp_syn());
    assert!(!parsed_data.payload.is_empty());
    assert!(!parsed_data.is_tcp_fin());

    // Build proxy response
    let response_payload = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
    let server_response = build_tcp_packet(
        [104, 1, 219, 8].into(),
        [10, 0, 0, 2].into(),
        80,
        49152,
        50001,  // server_next_seq
        1001 + parsed_data.payload.len() as u32,  // ack = seq + data_len
        0x18,   // ACK | PSH
        response_payload,
    );

    let parsed_resp = ParsedPacket::parse(&server_response).unwrap();
    assert_eq!(parsed_resp.payload, response_payload.to_vec());
}

#[test]
fn test_build_tcp_packet_minimal() {
    // Verify the simplest possible packet
    let pkt = build_tcp_packet(
        [192, 168, 1, 1].into(),
        [10, 0, 0, 1].into(),
        12345,
        443,
        1,
        2,
        0x10, // ACK only
        b"hello",
    );
    let parsed = ParsedPacket::parse(&pkt).unwrap();
    assert_eq!(parsed.payload, b"hello");
    assert_eq!(parsed.ip.source.to_string(), "192.168.1.1");
    assert_eq!(parsed.ip.destination.to_string(), "10.0.0.1");
}
