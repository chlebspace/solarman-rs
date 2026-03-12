use thiserror::Error;

#[cfg(feature = "codec")]
mod codec;
#[cfg(test)]
mod tests;

/// Error type used by the library.
#[derive(Error, Debug, PartialEq)]
pub enum Error {
    #[error("invalid checksum (expected {0:02X?} but got {1:02X?})")]
    Checksum(u8, u8),
    #[error("packet does not contain header")]
    NoHeader,
    #[error("start or end byte missing in packet")]
    Malformed,
}

pub type Result<T> = std::result::Result<T, Error>;

const CTL_REQUEST: u16 = 0x4510;
const CTL_RESPONSE: u16 = 0x1510;

fn write_header(
    buf: &mut [u8; 11],
    control_code: u16,
    id: u8,
    seq: u8,
    payload_len: u16,
    serial: u32,
) {
    let payload_len_bytes = payload_len.to_le_bytes();
    let serial_bytes = serial.to_le_bytes();
    let cc_bytes = (control_code as u16).to_le_bytes();
    *buf = [
        0xA5,                 // start
        payload_len_bytes[0], // payload length
        payload_len_bytes[1], //
        cc_bytes[0],          // control code
        cc_bytes[1],          //
        id,                   // id
        seq,                  // seq
        serial_bytes[0],      // logger serial
        serial_bytes[1],
        serial_bytes[2],
        serial_bytes[3],
    ];
}

fn solarman_checksum(buf: &[u8]) -> u8 {
    buf.iter().fold(0, |i, b| i.wrapping_add(*b))
}

/// Request packet in client mode.
#[derive(Debug, PartialEq)]
pub struct RequestPacket {
    /// The response to this request will have the same ID.
    pub id: u8,
    /// Sequence number. Can always be 0x00 for outgoing requests.
    pub seq: u8,
    /// Serial number of the data logging stick.
    pub serial: u32,
    /// Modbus RTU payload. Encoding will panic if this exceeds 256 bytes.
    pub modbus_payload: Vec<u8>,
}

impl RequestPacket {
    fn encode(&self, buf: &mut [u8]) {
        let modbus_len = self.modbus_payload.len();
        assert!(
            modbus_len <= 256,
            "modbus payload cannot be longer than 256 bytes, got {modbus_len}"
        );

        let payload_len = (15 + modbus_len) as u16;
        write_header(
            (&mut buf[..11]).try_into().unwrap(),
            CTL_REQUEST,
            self.id,
            self.seq,
            payload_len,
            self.serial,
        );

        let trailer_pos = 26 + modbus_len;

        buf[11..26].copy_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        buf[26..trailer_pos].copy_from_slice(&self.modbus_payload);

        let checksum: u8 = solarman_checksum(&buf[1..trailer_pos]);
        buf[trailer_pos..].copy_from_slice(&[checksum, 0x15]);
    }

    /// Encodes the packet to a slice
    pub fn encode_to_slice(&self, slice: &mut [u8]) -> usize {
        let size = self.size();
        let slice_len = slice.len();
        assert!(
            slice_len >= size,
            "buffer too small, expected at least {size} but got {slice_len}",
        );
        self.encode(slice);
        size
    }

    /// Encodes the packet to a `Vec<u8>` (convenience wrapper for `size` and `encode_to_slice`)
    pub fn encode_to_vec(&self) -> Vec<u8> {
        let mut vec = vec![0u8; self.size()];
        self.encode(&mut vec);
        vec
    }

    /// Calculates the size of the packet in bytes.
    pub fn size(&self) -> usize {
        11 /* hdr */ + 15 /* payload */ + self.modbus_payload.len() + 2 /* trailer */
    }
}

/// Response packet in client mode.
#[derive(Debug, PartialEq)]
pub struct ResponsePacket {
    pub id: u8,
    pub seq: u8,
    pub serial: u32,
    pub frame_type: u8,
    pub status: u8,
    pub total_working_time: u32,
    pub power_on_time: u32,
    pub offset_time: u32,
    pub modbus_payload: Vec<u8>,
}

/// Unparsed packet (possibly of unknown kind).
#[derive(Debug, PartialEq)]
pub struct RawPacket {
    pub id: u8,
    pub seq: u8,
    pub serial: u32,
    pub control_code: u16,
    pub raw_payload: Vec<u8>,
}

/// Enum returned as a result of packet parsing.
#[derive(Debug, PartialEq)]
pub enum ParsedPacket {
    Response(ResponsePacket),
    // TODO: handle heartbeat
    Unknown(RawPacket),
}

/// Parse a Solarman packet.
///
/// # Return value
/// On success, the return value is the parsed packet and its complete length.
/// If the buffer does not contain enough data, the return value is Ok(None).
pub fn parse(buf: &[u8]) -> Result<Option<(ParsedPacket, usize)>> {
    let buf_len = buf.len();
    if buf_len < 11 {
        return Ok(None);
    }
    if buf[0] != 0xA5 {
        return Err(Error::Malformed);
    }
    let payload_len = u16::from_le_bytes([buf[1], buf[2]]) as usize;
    let frame_len = 11 + payload_len + 2;
    println!("buf: {buf_len}; payload: {payload_len}; frame: {frame_len}");
    if buf_len < frame_len {
        return Ok(None);
    }
    let trailer_pos = frame_len - 2;
    if buf[trailer_pos + 1] != 0x15 {
        return Err(Error::Malformed);
    }

    let checksum = solarman_checksum(&buf[1..trailer_pos]);
    if buf[trailer_pos] != checksum {
        return Err(Error::Checksum(buf[trailer_pos], checksum));
    }

    let control_code = u16::from_le_bytes([buf[3], buf[4]]);
    let parsed_packet = match control_code {
        CTL_RESPONSE => {
            if payload_len < 14 {
                return Err(Error::Malformed);
            }
            ParsedPacket::Response(ResponsePacket {
                id: buf[5],
                seq: buf[6],
                serial: u32::from_le_bytes([buf[7], buf[8], buf[9], buf[10]]),
                frame_type: buf[11],
                status: buf[12],
                total_working_time: u32::from_le_bytes([buf[13], buf[14], buf[15], buf[16]]),
                power_on_time: u32::from_le_bytes([buf[17], buf[18], buf[19], buf[20]]),
                offset_time: u32::from_le_bytes([buf[21], buf[22], buf[23], buf[24]]),
                modbus_payload: Vec::from(&buf[25..trailer_pos]),
            })
        }
        CTL_REQUEST => unimplemented!(),
        _ => ParsedPacket::Unknown(RawPacket {
            id: buf[5],
            seq: buf[6],
            serial: u32::from_le_bytes([buf[7], buf[8], buf[9], buf[10]]),
            control_code,
            raw_payload: Vec::from(&buf[11..trailer_pos]),
        }),
    };

    Ok(Some((parsed_packet, frame_len)))
}
