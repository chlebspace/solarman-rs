#![cfg_attr(docsrs, feature(doc_cfg))]

use std::io;
use thiserror::Error;

#[cfg(feature = "codec")]
mod codec;
#[cfg(feature = "codec")]
pub use codec::*;
#[cfg(test)]
mod tests;

/// Error type used by the library.
#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
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

fn solarman_checksum(buf: &[u8]) -> u8 {
    buf.iter().fold(0, |i, b| i.wrapping_add(*b))
}

pub trait PacketEncode {
    /// Encodes the packet to a byte buffer.
    /// The slice must have at least `size()` bytes of space.
    fn encode(&self, buf: &mut [u8]);
    /// Calculates the size of the packet in bytes.
    fn size(&self) -> u16;
    /// Returns the control code corresponding to the packet.
    fn control_code(&self) -> u16;
}

/// Request packet in client mode.
#[derive(Debug, PartialEq)]
pub struct RequestPacket {
    /// Modbus RTU payload. Encoding will panic if this exceeds 256 bytes.
    pub modbus_payload: Box<[u8]>,
}

/// Response packet in client mode.
#[derive(Debug, PartialEq)]
pub struct ResponsePacket {
    pub frame_type: u8,
    pub status: u8,
    pub total_working_time: u32,
    pub power_on_time: u32,
    pub offset_time: u32,
    pub modbus_payload: Box<[u8]>,
}

/// Enum returned as a result of packet parsing.
#[derive(Debug, PartialEq)]
pub enum ParsedPacket {
    Response(ResponsePacket),
    // TODO: handle heartbeat
    Unknown((u16, Box<[u8]>)),
}

#[derive(Debug, PartialEq)]
pub struct Frame<P> {
    /// Local sequence number.
    /// All incoming packets will have the same value here as the most recent outgoing frame.
    pub local_seq: u8,
    /// Remote sequence number.
    /// This is incremented by the logging stick for every frame **globally** (including Cloud and other connections).
    ///
    /// For outgoing frames this can be 0x00, the stick doesn't seem to care.
    pub remote_seq: u8,
    /// Serial number of the data logging stick.
    pub serial: u32,
    pub packet: P,
}

impl PacketEncode for RequestPacket {
    fn encode(&self, buf: &mut [u8]) {
        buf[..15].copy_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        buf[15..].copy_from_slice(&self.modbus_payload);
    }

    fn size(&self) -> u16 {
        15 + self.modbus_payload.len() as u16
    }

    fn control_code(&self) -> u16 {
        CTL_REQUEST
    }
}

impl<P: PacketEncode> Frame<P> {
    pub fn encode(&self, buf: &mut [u8]) {
        let packet_sz = self.packet.size();
        let frame_sz = 11 + packet_sz as usize + 2;
        let buf_sz = buf.len();
        assert!(
            buf_sz >= frame_sz,
            "buffer too small, expected at least {frame_sz} bytes but got {buf_sz}"
        );

        // write header
        let packet_sz_bytes = packet_sz.to_le_bytes();
        let serial_bytes = self.serial.to_le_bytes();
        let cc_bytes = self.packet.control_code().to_le_bytes();
        buf[..11].copy_from_slice(&[
            0xA5,               // start
            packet_sz_bytes[0], // payload length
            packet_sz_bytes[1], //
            cc_bytes[0],        // control code
            cc_bytes[1],        //
            self.local_seq,     // local seq
            self.remote_seq,    // remote seq
            serial_bytes[0],    // logger serial
            serial_bytes[1],
            serial_bytes[2],
            serial_bytes[3],
        ]);

        // write payload (packet)
        let trailer_pos = frame_sz - 2;
        self.packet.encode(&mut buf[11..trailer_pos]);

        // write trailer
        let checksum: u8 = solarman_checksum(&buf[1..trailer_pos]);
        buf[trailer_pos..frame_sz].copy_from_slice(&[checksum, 0x15]);
    }

    pub fn encode_to_vec(&self) -> Vec<u8> {
        let mut vec = vec![0; self.size()];
        self.encode(&mut vec);
        vec
    }

    pub fn size(&self) -> usize {
        11 + self.packet.size() as usize + 2
    }
}

/// Parse a Solarman frame.
///
/// # Return value
/// On success, the return value is the parsed frame and its complete length.
/// If the buffer does not contain enough data, the return value is Ok(None).
pub fn parse_frame(buf: &[u8]) -> Result<Option<(Frame<ParsedPacket>, usize)>> {
    let buf_len = buf.len();
    if buf_len < 11 {
        return Ok(None);
    }
    if buf[0] != 0xA5 {
        return Err(Error::Malformed);
    }

    let packet_sz = u16::from_le_bytes([buf[1], buf[2]]) as usize;
    let frame_sz = 11 + packet_sz + 2;
    if buf_len < frame_sz {
        return Ok(None);
    }

    let trailer_pos = frame_sz - 2;
    if buf[trailer_pos + 1] != 0x15 {
        return Err(Error::Malformed);
    }

    let checksum = solarman_checksum(&buf[1..trailer_pos]);
    if buf[trailer_pos] != checksum {
        return Err(Error::Checksum(buf[trailer_pos], checksum));
    }

    let packet = match u16::from_le_bytes([buf[3], buf[4]]) {
        CTL_RESPONSE => {
            if packet_sz < 14 {
                return Err(Error::Malformed);
            }
            ParsedPacket::Response(ResponsePacket {
                frame_type: buf[11],
                status: buf[12],
                total_working_time: u32::from_le_bytes([buf[13], buf[14], buf[15], buf[16]]),
                power_on_time: u32::from_le_bytes([buf[17], buf[18], buf[19], buf[20]]),
                offset_time: u32::from_le_bytes([buf[21], buf[22], buf[23], buf[24]]),
                modbus_payload: buf[25..trailer_pos].into(),
            })
        }
        control_code => ParsedPacket::Unknown((control_code, buf[11..trailer_pos].into())),
    };

    let frame = Frame {
        local_seq: buf[5],
        remote_seq: buf[6],
        serial: u32::from_le_bytes([buf[7], buf[8], buf[9], buf[10]]),
        packet,
    };

    Ok(Some((frame, frame_sz)))
}
