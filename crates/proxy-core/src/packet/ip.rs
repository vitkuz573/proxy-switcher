use std::net::Ipv4Addr;

#[derive(Debug, Clone)]
pub struct IpHeader {
    pub version: u8,
    pub ihl: u8,
    pub total_length: u16,
    pub identification: u16,
    pub flags: u8,
    pub fragment_offset: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub checksum: u16,
    pub source: Ipv4Addr,
    pub destination: Ipv4Addr,
}

impl IpHeader {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 20 {
            return Err("Packet too short");
        }
        let v_ihl = data[0];
        if (v_ihl >> 4) != 4 {
            return Err("Not IPv4");
        }
        let ihl = ((v_ihl & 0x0F) * 4) as usize;
        if ihl < 20 || data.len() < ihl {
            return Err("Bad IHL");
        }
        Ok(Self {
            version: 4,
            ihl: ihl as u8,
            total_length: u16::from_be_bytes([data[2], data[3]]),
            identification: u16::from_be_bytes([data[4], data[5]]),
            flags: data[6] >> 5,
            fragment_offset: u16::from_be_bytes([data[6] & 0x1F, data[7]]) & 0x1FFF,
            ttl: data[8],
            protocol: data[9],
            checksum: u16::from_be_bytes([data[10], data[11]]),
            source: Ipv4Addr::new(data[12], data[13], data[14], data[15]),
            destination: Ipv4Addr::new(data[16], data[17], data[18], data[19]),
        })
    }

    pub fn header_len(&self) -> usize {
        self.ihl as usize
    }
}
