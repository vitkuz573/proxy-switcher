pub mod ip;
pub mod tcp;

use ip::IpHeader;
use tcp::TcpHeader;
use std::net::Ipv4Addr;

#[derive(Debug)]
pub struct ParsedPacket {
    pub ip: IpHeader,
    pub tcp: Option<TcpHeader>,
    pub payload: Vec<u8>,
}

impl ParsedPacket {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        let ip = IpHeader::parse(data)?;
        let ip_end = ip.header_len();

        let (tcp, payload_start) = if ip.protocol == 6 {
            let tcp_data = &data[ip_end..];
            let tcp = TcpHeader::parse(tcp_data)?;
            let tcp_len = tcp.header_len();
            (Some(tcp), ip_end + tcp_len)
        } else {
            (None, ip_end)
        };

        let payload = if payload_start < data.len() {
            data[payload_start..].to_vec()
        } else {
            Vec::new()
        };

        Ok(Self { ip, tcp, payload })
    }

    pub fn is_tcp_syn(&self) -> bool {
        self.tcp.as_ref().is_some_and(|t| t.flags.syn && !t.flags.ack)
    }

    pub fn is_tcp_fin(&self) -> bool {
        self.tcp.as_ref().is_some_and(|t| t.flags.fin || t.flags.rst)
    }
}

/// Build a response IP packet swapping src/dst and setting payload
pub fn build_response_packet(original: &ParsedPacket, payload: &[u8]) -> Vec<u8> {
    let ip = &original.ip;
    let tcp = match &original.tcp {
        Some(t) => t,
        None => return Vec::new(),
    };

    let tcp_len = 20; // minimal TCP header, no options
    let ip_total = 20 + tcp_len + payload.len();

    let mut pkt = Vec::with_capacity(ip_total);
    // IP header
    pkt.push(0x45); // v4, ihl=20
    pkt.push(0x00); // DSCP
    pkt.extend_from_slice(&(ip_total as u16).to_be_bytes());
    pkt.extend_from_slice(&ip.identification.wrapping_add(1).to_be_bytes());
    pkt.push(0x40); // flags=0, frag_offset=0
    pkt.push(0x00);
    pkt.push(64); // TTL
    pkt.push(6); // TCP
    pkt.extend_from_slice(&[0x00, 0x00]); // checksum = 0 (computed later)
    // Swap src/dst
    pkt.extend_from_slice(&original.ip.destination.octets());
    pkt.extend_from_slice(&original.ip.source.octets());

    // TCP header (swapped ports, ack flag)
    pkt.extend_from_slice(&tcp.destination_port.to_be_bytes());
    pkt.extend_from_slice(&tcp.source_port.to_be_bytes());
    pkt.extend_from_slice(&tcp.acknowledgment_number.to_be_bytes());
    pkt.extend_from_slice(&tcp.sequence_number.wrapping_add(1).to_be_bytes());
    pkt.push(0x50); // data offset = 20
    pkt.push(0x10); // ACK
    pkt.extend_from_slice(&(65535u16).to_be_bytes()); // window
    pkt.extend_from_slice(&[0x00, 0x00]); // checksum placeholder
    pkt.extend_from_slice(&[0x00, 0x00]); // urgent

    // Payload
    pkt.extend_from_slice(payload);

    // Compute IP checksum
    let ip_csum = ip_checksum(&pkt[..20]);
    pkt[10] = (ip_csum >> 8) as u8;
    pkt[11] = (ip_csum & 0xFF) as u8;

    // Compute TCP checksum (with pseudo header)
    let tcp_csum = tcp_checksum(
        &original.ip.destination,
        &original.ip.source,
        &pkt[20..],
    );
    pkt[20 + 16] = (tcp_csum >> 8) as u8;
    pkt[20 + 17] = (tcp_csum & 0xFF) as u8;

    pkt
}

fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in data.chunks(2) {
        let word = u16::from_be_bytes([chunk[0], if chunk.len() > 1 { chunk[1] } else { 0 }]);
        sum = sum.wrapping_add(word as u32);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn tcp_checksum(src: &Ipv4Addr, dst: &Ipv4Addr, segment: &[u8]) -> u16 {
    let pseudo_len = 12 + segment.len();
    let mut buf = Vec::with_capacity(pseudo_len);
    buf.extend_from_slice(&src.octets());
    buf.extend_from_slice(&dst.octets());
    buf.push(0); // zero
    buf.push(6); // protocol TCP
    let len_bytes = (segment.len() as u16).to_be_bytes();
    buf.extend_from_slice(&len_bytes);
    buf.extend_from_slice(segment);

    // Zero out the checksum field in the copied segment
    let tcp_start = 12;
    if buf.len() >= tcp_start + 18 {
        buf[tcp_start + 16] = 0;
        buf[tcp_start + 17] = 0;
    }

    ip_checksum(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_syn_packet() -> Vec<u8> {
        let mut pkt = vec![
            0x45, 0x00, 0x00, 0x28,
            0x00, 0x01, 0x00, 0x00,
            0x40, 0x06, 0x00, 0x00,
            0x0a, 0x00, 0x00, 0x02,
            0x68, 0x01, 0xdb, 0x08,
        ];
        pkt.extend_from_slice(&[
            0xc0, 0x00, 0x00, 0x50,
            0x00, 0x00, 0x00, 0x64,
            0x00, 0x00, 0x00, 0x00,
            0x50, 0x02, 0x71, 0x10,
            0x00, 0x00, 0x00, 0x00,
        ]);
        pkt
    }

    #[test]
    fn test_parse_syn() {
        let p = ParsedPacket::parse(&make_syn_packet()).unwrap();
        assert!(p.is_tcp_syn());
        assert_eq!(p.ip.source.to_string(), "10.0.0.2");
        assert_eq!(p.ip.destination.to_string(), "104.1.219.8");
        assert_eq!(p.tcp.as_ref().unwrap().source_port, 49152);
        assert_eq!(p.tcp.as_ref().unwrap().destination_port, 80);
    }

    #[test]
    fn test_build_response() {
        let orig = ParsedPacket::parse(&make_syn_packet()).unwrap();
        let resp = build_response_packet(&orig, b"HTTP/1.1 200 OK\r\n");
        assert!(!resp.is_empty());
        // Check src/dst are swapped
        let parsed = ParsedPacket::parse(&resp).unwrap();
        assert_eq!(parsed.ip.source.to_string(), "104.1.219.8");
        assert_eq!(parsed.ip.destination.to_string(), "10.0.0.2");
        assert_eq!(parsed.tcp.as_ref().unwrap().source_port, 80);
        assert_eq!(parsed.tcp.as_ref().unwrap().destination_port, 49152);
    }
}
