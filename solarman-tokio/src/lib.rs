use std::time::Duration;

use futures::{SinkExt, StreamExt};
use solarman_protocol::{ParsedPacket, SolarmanCodec};
use thiserror::Error;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpStream, ToSocketAddrs},
};
use tokio_util::codec::Framed;

/// Error type used by the library.
#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("solarman protocol error: {0}")]
    Solarman(#[from] solarman_protocol::Error),
    #[error("modbus request encoding error: {0}")]
    ModbusRequest(#[from] modbus_rtu::error::RequestPacketError),
    #[error("modbus response parsing error: {0}")]
    ModbusResponse(#[from] modbus_rtu::error::ResponsePacketError),
    #[error("modbus exception: {0}")]
    ModbusException(modbus_rtu::Exception),
    #[error("the stream ended unexpectedly")]
    UnexpectedEof,
    #[error("unexpected modbus response received")]
    UnexpectedResponse(modbus_rtu::Response),
    #[error("response doesn't match request serial (wrong serial number)")]
    BadSerial,
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Client {
    seq: u8,
    serial: u32,
    modbus_id: u8,
    stream: Framed<TcpStream, SolarmanCodec>,
}

impl Client {
    pub async fn connect<A: ToSocketAddrs>(
        addr: A,
        serial: u32,
        modbus_slave_id: u8,
    ) -> Result<Self> {
        let tcp = TcpStream::connect(addr).await?;
        Ok(Self {
            seq: 0,
            serial,
            modbus_id: modbus_slave_id,
            stream: Framed::new(tcp, SolarmanCodec),
        })
    }

    async fn execute_modbus(
        &mut self,
        function: &modbus_rtu::Function,
    ) -> Result<modbus_rtu::Response> {
        // TODO: implement timeout?
        let modbus_req = modbus_rtu::Request::new(self.modbus_id, function, Duration::ZERO);
        self.seq = self.seq.wrapping_add(1);
        let solarman_req = solarman_protocol::Frame {
            local_seq: self.seq,
            remote_seq: 0,
            serial: self.serial,
            packet: solarman_protocol::RequestPacket {
                modbus_payload: modbus_req.to_bytes()?,
            },
        };

        tracing::debug!("sending solarman request: {solarman_req:?}");
        self.stream.send(solarman_req).await?;

        loop {
            let solarman_frame = self.stream.next().await.ok_or(Error::UnexpectedEof)??;
            tracing::debug!("received solarman response: {solarman_frame:?}");

            if solarman_frame.serial != self.serial {
                return Err(Error::BadSerial);
            }

            if solarman_frame.local_seq != self.seq {
                tracing::debug!("frame with invalid seq, dropping {solarman_frame:?}");
                continue;
            }

            match solarman_frame.packet {
                ParsedPacket::Response(resp) => {
                    let modbus_resp =
                        modbus_rtu::Response::from_bytes(&modbus_req, &resp.modbus_payload)?;
                    tracing::debug!("modbus response decoded: {modbus_resp:?}");
                    return Ok(modbus_resp);
                }
                ParsedPacket::Unknown((code, payload)) => {
                    tracing::warn!(
                        "unknown solarman packet received: {code:02X} {payload:02X?}, skipping"
                    );
                }
            }
        }
    }

    pub async fn write_coil(&mut self, address: u16, value: bool) -> Result<()> {
        match self
            .execute_modbus(&modbus_rtu::Function::WriteSingleCoil { address, value })
            .await?
        {
            modbus_rtu::Response::Success => Ok(()),
            modbus_rtu::Response::Exception(exception) => Err(Error::ModbusException(exception)),
            response => Err(Error::UnexpectedResponse(response)),
        }
    }

    pub async fn write_multiple_coils(
        &mut self,
        starting_address: u16,
        values: Box<[bool]>,
    ) -> Result<()> {
        match self
            .execute_modbus(&modbus_rtu::Function::WriteMultipleCoils {
                starting_address,
                value: values,
            })
            .await?
        {
            modbus_rtu::Response::Success => Ok(()),
            modbus_rtu::Response::Exception(exception) => Err(Error::ModbusException(exception)),
            response => Err(Error::UnexpectedResponse(response)),
        }
    }

    pub async fn write_register(&mut self, address: u16, value: u16) -> Result<()> {
        match self
            .execute_modbus(&modbus_rtu::Function::WriteSingleRegister { address, value })
            .await?
        {
            modbus_rtu::Response::Success => Ok(()),
            modbus_rtu::Response::Exception(exception) => Err(Error::ModbusException(exception)),
            response => Err(Error::UnexpectedResponse(response)),
        }
    }

    pub async fn write_multiple_registers(
        &mut self,
        starting_address: u16,
        values: Box<[u16]>,
    ) -> Result<()> {
        match self
            .execute_modbus(&modbus_rtu::Function::WriteMultipleRegisters {
                starting_address,
                value: values,
            })
            .await?
        {
            modbus_rtu::Response::Success => Ok(()),
            modbus_rtu::Response::Exception(exception) => Err(Error::ModbusException(exception)),
            response => Err(Error::UnexpectedResponse(response)),
        }
    }

    pub async fn read_coils(
        &mut self,
        starting_address: u16,
        quantity: u16,
    ) -> Result<Box<[bool]>> {
        match self
            .execute_modbus(&modbus_rtu::Function::ReadCoils {
                starting_address,
                quantity,
            })
            .await?
        {
            modbus_rtu::Response::Status(items) => Ok(items),
            modbus_rtu::Response::Exception(exception) => Err(Error::ModbusException(exception)),
            response => Err(Error::UnexpectedResponse(response)),
        }
    }

    pub async fn read_discrete_inputs(
        &mut self,
        starting_address: u16,
        quantity: u16,
    ) -> Result<Box<[bool]>> {
        match self
            .execute_modbus(&modbus_rtu::Function::ReadDiscreteInputs {
                starting_address,
                quantity,
            })
            .await?
        {
            modbus_rtu::Response::Status(items) => Ok(items),
            modbus_rtu::Response::Exception(exception) => Err(Error::ModbusException(exception)),
            response => Err(Error::UnexpectedResponse(response)),
        }
    }

    pub async fn read_input_registers(
        &mut self,
        starting_address: u16,
        quantity: u16,
    ) -> Result<Box<[u16]>> {
        match self
            .execute_modbus(&modbus_rtu::Function::ReadInputRegisters {
                starting_address,
                quantity,
            })
            .await?
        {
            modbus_rtu::Response::Value(items) => Ok(items),
            modbus_rtu::Response::Exception(exception) => Err(Error::ModbusException(exception)),
            response => Err(Error::UnexpectedResponse(response)),
        }
    }

    pub async fn read_holding_registers(
        &mut self,
        starting_address: u16,
        quantity: u16,
    ) -> Result<Box<[u16]>> {
        match self
            .execute_modbus(&modbus_rtu::Function::ReadHoldingRegisters {
                starting_address,
                quantity,
            })
            .await?
        {
            modbus_rtu::Response::Value(items) => Ok(items),
            modbus_rtu::Response::Exception(exception) => Err(Error::ModbusException(exception)),
            response => Err(Error::UnexpectedResponse(response)),
        }
    }

    pub async fn shutdown(self) -> Result<()> {
        self.stream
            .into_inner()
            .shutdown()
            .await
            .map_err(Into::into)
    }
}
