use core::{fmt, str};
use std::{collections::HashMap, error::Error, sync::mpsc::{Receiver, TryRecvError}, time::Duration};

use mio::Token;
use sha1::{Digest, Sha1};
use base64::prelude::*;

use crate::{helpers::Write2, serve_client, ClientManifest, Protocol, Vfs, TLS::TLStream};

pub fn compute_sec_websocket_accept(key: &str) -> String {
    let mut sha1 = Sha1::new();
    sha1.update(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11"));
    BASE64_STANDARD.encode(sha1.finalize())
}

pub struct WebSocket {
    incoming_message: Option<Message>,
    //TODO: change this
    receiver: Receiver<TLStream>,
    queue: mio::Events,
    poll: mio::Poll,
    clients: ClientManifest,
}

impl WebSocket {
    pub fn new(receiver: Receiver<TLStream>) -> Self {
        Self {
            incoming_message: None, 
            receiver, 
            queue: mio::Events::with_capacity(128),
            poll: mio::Poll::new().unwrap(),
            clients: ClientManifest::new(128),
        }
    }
    fn welcome_new_clients(&mut self) {
        loop {
            match self.receiver.try_recv() {
                Ok(stream) => {
                    println!("WEBSOCKETS: added 1 client");
                    self.clients.insert(stream, Protocol::WEBSOCKET, self.poll.registry()).unwrap();
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => panic!(),
            }
        }
    }
    pub fn read_message(&mut self) -> Result<(Token, Message), WebSocketError> {
        use WebSocketError::*;

        self.welcome_new_clients();
        self.poll.poll(&mut self.queue, Some(Duration::ZERO)).unwrap();
        for event in self.queue.iter().cloned() {
            let client_id = event.token();
            match serve_client(&mut Vfs(HashMap::new()), &event, &mut self.clients) {
                Ok(0) => {},
                Ok(_) => {
                    let message = self.parse_message(client_id)?;
                    let client = self.clients.get(client_id).unwrap();
                    client.buffer.clear();
                    return Ok((client_id, message));
                },
                Err(e) => {
                    println!("WEBSOCKET: dropped client {} on account of error {e}", client_id.0);
                    self.clients.remove(client_id);
                }
            };
        };
        Err(WOULD_BLOCK)
    }
    pub fn send_binary(&mut self, message: &[u8], client: Token) -> Result<(), WebSocketError> {
        self.write_message(Message::Binary(message.to_owned()), client)
    }
    pub fn send_text(&mut self, message: &str, client: Token) -> Result<(), WebSocketError> {
        self.write_message(Message::Text(message.to_owned()), client)
    }
    fn parse_message(&mut self, client: Token) -> Result<Message, WebSocketError> {
        use OPCODE::*;
        use WebSocketError::*;
        let client = self.clients.get(client).unwrap();
        let mut buf = &client.buffer[..];
        loop {
            let (frame, rest) = Self::read_frame(buf)?;
            buf = rest;
            match frame.opcode {
                Continuation => {
                    if let Some(message) = self.incoming_message.take() {
                        self.incoming_message = match message {
                            Message::Text(t) => Message::Text(frame.unmask_into_text(t)?),
                            Message::Binary(b) => Message::Binary(frame.unmask_into_binary(b)),
                        }.into();
                        if frame.fin { 
                            client.buffer.clear();
                            return Ok(self.incoming_message.take().unwrap()); 
                        }
                        else { continue }
                    }
                    else { return Err(BAD_CONTINUE) }
                }
                Text => {
                    if self.incoming_message.is_some() { return Err(CUTTING_IN) }
                    self.incoming_message = Some(Message::Text(frame.unmask_into_text(String::with_capacity(256))?));
                    if frame.fin { 
                        client.buffer.clear();
                        return Ok(self.incoming_message.take().unwrap()); 
                    }
                    else { continue }
                },
                Binary => {
                    if self.incoming_message.is_some() { return Err(CUTTING_IN) }
                    self.incoming_message = Some(Message::Binary(frame.unmask_into_binary(Vec::with_capacity(256))));
                    if frame.fin { 
                        client.buffer.clear();
                        return Ok(self.incoming_message.take().unwrap()); 
                    }
                    else { continue }
                }
                Close => { return Err(UNIMPLEMENTED) },
                Ping => { return Err(UNIMPLEMENTED) },
                Pong => { return Err(UNIMPLEMENTED) },
            }
        }
    }
    fn read_frame(data: &[u8]) -> Result<(Frame, &[u8]), WebSocketError> {
        use WebSocketError::*;

        if data.len() < (1 + 1 + 4) { return Err(TOO_SHORT) }
    
        let fin = (data[0] >> 7) > 0;
        if ((data[0] >> 4) & 0b111) != 0 { return Err(UNRESERVED) }
        let opcode = OPCODE::parse(data[0] & 0b1111)?;
        if ((data[1] >> 7) & 0b1) == 0 { return Err(UNMASKED) }
        let mask_key_offset: usize;
        let payload_len = match (data[1] & 0b01111111) {
            l if l < 126 => {
                mask_key_offset = 2;
                l as u64
            },
            126 => {
                mask_key_offset = 4;
                if data.len() < 8 { return Err(TOO_SHORT) }
                u16::from_be_bytes(data[2..4].try_into().unwrap()) as u64
            },
            127 => {
                mask_key_offset = 10;
                if data.len() < 14 { return Err(TOO_SHORT) }
                u64::from_be_bytes(data[2..10].try_into().unwrap())
            }
            _ => unreachable!(),
        };
        let mask_key: [u8;4] = data[mask_key_offset..mask_key_offset+4].try_into().unwrap();
        let payload = &data[mask_key_offset+4..];
        println!("PAYLOAD_LENGTH = {payload_len}, ACTUAL_LENGTH = {}", payload.len());
        let (payload, rest) = payload.split_at_checked(payload_len as usize).ok_or(WOULD_BLOCK)?;
    
        let frame = Frame {
            fin,
            opcode,
            mask_key: Some(mask_key),
            payload,
        };
    
        println!("Frame: fin = {}, opcode = {:?}", frame.fin, frame.opcode);
    
        Ok((frame, rest))
    }
    fn write_message(&mut self, msg: Message, client: Token) -> Result<(), WebSocketError> {
        let frame = msg.to_frame();
        let len = frame.payload.len();
        let mut buf = Vec::<u8>::with_capacity(len + 2);
        buf.push((1 << 7) | (frame.opcode as u8));

        const limit1: usize = 126;
        const limit2: usize = 1 << 16;
        match len {
            0..limit1 => buf.push(len as u8),
            limit1..limit2 => {
                buf.push(126);
                buf.append(&mut (len as u16).to_be_bytes().to_vec());
            }
            limit2.. => {
                buf.push(127);
                buf.append(&mut (len as u64).to_be_bytes().to_vec());
            }
        }
        buf.append(&mut frame.payload.to_owned());
        if let Some(client) = self.clients.get(client) {
            let mut bytes = 0;
            while bytes < buf.len() {
                let (bytes_writ, err) = client.stream.write2(&buf[bytes..]);
                if let Err(e) = err { panic!("{e}")};
                println!("WEBSOCKET: wrote {} bytes to client {}", bytes_writ, client.token.0);
                bytes += bytes_writ;
            }
        } else { panic!("WebSocket::write_message: invalid client ID {:?}", client.0) }
        Ok(())
    }
    
}

#[derive(Debug)]
#[derive(PartialEq)]
pub enum WebSocketError {
    TOO_SHORT,
    UNRESERVED,
    UNMASKED,
    BAD_OPCODE,
    BAD_CONTINUE,
    CUTTING_IN,
    EXPECTED_UTF8,
    CLOSED_BY_CLIENT,
    WOULD_BLOCK,
    UNIMPLEMENTED,
}

impl fmt::Display for WebSocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for WebSocketError{}

#[derive(Debug)]
enum OPCODE {
    Continuation = 0b0,
    Text    = 0b0001,
    Binary  = 0b0010,
    Close   = 0b1000,
    Ping    = 0b1001,
    Pong    = 0b1010,
}

impl OPCODE {
    fn parse(value: u8) -> Result<Self, WebSocketError> {
        use OPCODE::*;
        use WebSocketError::*;

        let code = match value {
            0b0000 => Continuation,
            0b0001 => Text,
            0b0010 => Binary,
            0b1000 => Close,
            0b1001 => Ping,
            0b1010 => Pong,
            _ => return Err(BAD_OPCODE),
        };
        Ok(code)
    }
}

struct Frame<'buf> {
    pub fin: bool,
    pub opcode: OPCODE,
    pub mask_key: Option<[u8;4]>,
    pub payload: &'buf [u8],
}

impl Frame<'_> {
    pub fn unmask_into_binary(&self, mut buffer: Vec<u8>) -> Vec<u8> {
        assert!(buffer.len() <= self.payload.len());

        if let Some(mask_key) = self.mask_key {
            for (index, byte)  in self.payload.iter().enumerate() {
                buffer.push(byte ^ mask_key[index % 4])
            }
        } else {
            buffer.append(&mut self.payload.to_owned());
        }
        buffer
    }
    pub fn unmask_into_text(&self, buffer: String) -> Result<String, WebSocketError> {
        use WebSocketError::*;
        assert!(buffer.len() <= self.payload.len());

        let mut buffer = buffer.into_bytes();
        let initial_length = buffer.len();
        if let Some(mask_key) = self.mask_key {
            for (index, byte)  in self.payload.iter().enumerate() {
                buffer.push(byte ^ mask_key[index % 4])
            }
        } else {
            buffer.append(&mut self.payload.to_owned());
        }
        let _ = str::from_utf8(&buffer[initial_length..]).map_err(|_e| EXPECTED_UTF8)?;
        return Ok( unsafe {String::from_utf8_unchecked(buffer)} )
    }
}

pub enum Message {
    Text(String),
    Binary(Vec<u8>),
}

impl Message {
    fn to_frame(&self) -> Frame {
        match self {
            Message::Binary(contents) => Frame {
                fin: true,
                opcode: OPCODE::Binary,
                mask_key: None,
                payload: contents,
            },
            Message::Text(contents) => Frame { 
                fin: true, 
                opcode: OPCODE::Text, 
                mask_key: None, 
                payload: contents.as_bytes(),
            }
        }
    }
}