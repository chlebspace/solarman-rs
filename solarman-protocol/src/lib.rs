use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {}

fn write_header(buf: &mut [u8; 11], id: u8, payload_len: u16, serial: u32) {
    let payload_len_bytes = payload_len.to_le_bytes();
    let serial_bytes = serial.to_le_bytes();
    *buf = [
        0xA5,                 // start
        payload_len_bytes[0], // payload length
        payload_len_bytes[1], //
        0x10,                 // control code
        0x45,                 //
        id,                   // id
        0x00,                 // seq
        serial_bytes[0],      // logger serial
        serial_bytes[1],
        serial_bytes[2],
        serial_bytes[3],
    ];
}

pub struct RequestPacket<'p> {
    /// The response for this request will have the same ID.
    pub id: u8,
    /// Serial number of the data logging stick.
    pub serial: u32,
    /// Modbus RTU payload. Encoding will panic if this exceeds 256 bytes.
    pub modbus_payload: &'p [u8],
}

impl<'p> RequestPacket<'p> {
    fn encode(&self, buf: &mut [u8]) {
        let modbus_len = self.modbus_payload.len();
        assert!(
            modbus_len <= 256,
            "modbus payload cannot be longer than 256 bytes, got {modbus_len}"
        );

        let payload_len = (15 + modbus_len) as u16;
        let header_section = (&mut buf[..11]).try_into().unwrap();
        write_header(header_section, self.id, payload_len, self.serial);

        let trailer_pos = 26 + modbus_len;

        buf[11..26].copy_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        buf[26..trailer_pos].copy_from_slice(self.modbus_payload);

        let checksum: usize = buf[1..trailer_pos].iter().fold(0, |i, b| i + *b as usize);

        buf[trailer_pos..].copy_from_slice(&[(checksum & 255) as u8, 0x15]);
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

    /// Encodes the packet to a `Vec<u8>` (convenience wrapper for `slice` and `encode_to_slice`)
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

pub struct ResponsePacket<'p> {
    modbus_payload: &'p [u8],
}

impl<'p> ResponsePacket<'p> {}

impl<'p> TryFrom<&'p [u8]> for ResponsePacket<'p> {
    type Error = Error;

    fn try_from(value: &'p [u8]) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[cfg(test)]
mod tests;
