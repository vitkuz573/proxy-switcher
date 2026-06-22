#[derive(Debug, Clone)]
pub struct TcpHeader {
    pub source_port: u16,
    pub destination_port: u16,
    pub sequence_number: u32,
    pub acknowledgment_number: u32,
    pub data_offset: u8,
    pub flags: TcpFlags,
    pub window_size: u16,
    pub checksum: u16,
    pub urgent_pointer: u16,
}

#[derive(Debug, Clone, Default)]
pub struct TcpFlags {
    pub fin: bool,
    pub syn: bool,
    pub rst: bool,
    pub psh: bool,
    pub ack: bool,
    pub urg: bool,
}

impl TcpHeader {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 20 {
            return Err("TCP header too short");
        }
        let doff = ((data[12] >> 4) * 4) as usize;
        if doff < 20 || data.len() < doff {
            return Err("Bad TCP data offset");
        }
        let f = data[13];
        Ok(Self {
            source_port: u16::from_be_bytes([data[0], data[1]]),
            destination_port: u16::from_be_bytes([data[2], data[3]]),
            sequence_number: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            acknowledgment_number: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
            data_offset: doff as u8,
            flags: TcpFlags {
                fin: (f & 0x01) != 0, syn: (f & 0x02) != 0,
                rst: (f & 0x04) != 0, psh: (f & 0x08) != 0,
                ack: (f & 0x10) != 0, urg: (f & 0x20) != 0,
            },
            window_size: u16::from_be_bytes([data[14], data[15]]),
            checksum: u16::from_be_bytes([data[16], data[17]]),
            urgent_pointer: u16::from_be_bytes([data[18], data[19]]),
        })
    }

    pub fn header_len(&self) -> usize {
        self.data_offset as usize
    }
}
