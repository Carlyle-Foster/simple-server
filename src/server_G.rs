use core::fmt;
use std::{collections::HashMap, io::{ErrorKind, Write}, marker::PhantomData, net::SocketAddr, sync::Arc, time::{Duration, Instant}};

use mio::{net::{TcpListener, TcpStream}, Events, Interest, Poll, Token};
use rustls::ServerConfig;

use crate::{helpers::{throw_reader_at_writer, Parser, SendTo}, Buffer, SERVER, TLS::TLStream, TLS2::StreamId};

pub struct Server_G<M, P, T, E, H> 
where 
    M: SendTo + Default,
    P: Parser<T, E> + Default,
    E: fmt::Display,
    H: Handshaker + Default,
{
    pub clients: HashMap<StreamId, Client<M, P, T, E, H>>,

    pub config: Arc<ServerConfig>,
    pub listener: TcpListener,
    pub poll: Poll,
    pub events: Events,
    pub heartbeat: Option<Duration>,
    pub last_beat: Instant,
    pub events_processed: usize,
    pub id_to_delete: StreamId,
    pub queued_disconnects: Vec<StreamId>,
    h: PhantomData<H>,
}

pub enum Notification<T> {
    SentMessage(StreamId, T),
    Disconnected(StreamId),
    Heartbeat,
}

pub trait Handshaker: SendTo {
    // returning None indicates a handshake error, if no progress is made just return an empty vec
    fn handshake<'b>(&mut self, buf: &'b [u8]) -> Option<(HandshakeStatus, &'b [u8])>;
}

pub enum HandshakeStatus {
    Waiting,
    Responding,
    Done,
}

impl<M, P, T, E, H> Server_G<M, P, T, E, H> 
where 
    M: SendTo + Default + From<Vec<u8>>,
    P: Parser<T, E> + Default,
    E: fmt::Display,
    H: Handshaker + Default,
{
    pub fn new(address: SocketAddr, config: ServerConfig) -> Self {
        let poll = Poll::new().unwrap();
        let registry = poll.registry();
        let mut listener = TcpListener::bind(address).unwrap();

        registry.register(&mut listener, Token(SERVER), Interest::READABLE | Interest::WRITABLE).unwrap();

        println!("HTTPSERVER: initializing server on (https://{}:{})", address.ip(), address.port());

        Self { 
            clients: HashMap::with_capacity(1028),
            config: Arc::new(config),
            listener,
            poll,
            events: Events::with_capacity(128),
            heartbeat: None,
            last_beat: Instant::now(),
            events_processed: 0,
            id_to_delete: 0,
            queued_disconnects: Vec::with_capacity(64),
            h: PhantomData,
        }
    }
    pub fn serve(&mut self) -> Notification<T> {
        loop {
            if let Some(id) = self.queued_disconnects.pop() {
                return Notification::Disconnected(id)
            }
            let mut time = None;
            let non_blocking = self.heartbeat == Some(Duration::ZERO);
            if let Some(beat) = self.heartbeat {
                let timer = beat.saturating_sub(self.last_beat.elapsed());
                self.last_beat = Instant::now();
                if timer.is_zero() && !non_blocking {
                    return Notification::Heartbeat
                }
                time = Some(timer)
            }
            if self.events.iter().nth(self.events_processed).is_none() {
                self.events_processed = 0;
                println!("timeout = {time:?}");
                match self.poll.poll(&mut self.events, time) {
                    Ok(_) => {},
                    Err(ref e) if e.kind() == ErrorKind::Interrupted => {},
                    Err(e) => panic!("{e}"),
                }
                if non_blocking && self.events.is_empty() { 
                    return Notification::Heartbeat
                }
            }
            while let Some(event) =  self.events.iter().nth(self.events_processed) {
                // println!("{:?}", event);
                self.events_processed += 1;
                let id = event.token().0 as StreamId;
                match id {
                    SERVER => {
                        match self.listener.accept() {
                            Ok((client, _)) => {
                                self.register(client);
                                println!("registered client")
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                            Err(e) => {
                                println!("TLServer: connection refused due to error accepting: {:?}", e);
                            },
                        }
                    }
                    _client => {
                        let client = self.clients.get_mut(&id).unwrap();
                        let stream = &mut client.stream;

                        // TLS layer handshaking
                        if stream.tls.is_handshaking() {
                            match client.stream.handshake() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::Interrupted => {},
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when handshaking: {e}");
                                    self.drop_client(id);
                                    // THE APPLICATION DOESN'T NEED TO KNOW
                                    let _ = self.queued_disconnects.pop();
                                },
                            };
                            continue
                        }
                        if event.is_writable() {
                            //TODO: this might be redundant
                            match stream.flush() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::ConnectionAborted => {
                                    self.drop_client(id);
                                    continue
                                },
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when flushing: {e}");
                                    self.drop_client(id);
                                    continue
                                },
                            }
                            if !client.is_handshaking {
                                match client.messenger.send_all(stream) {
                                    Ok(_) => {},
                                    Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                    Err(e) => {
                                        println!("HTTP_SERVER: dropped client on account of error when writing: {e}");
                                        self.drop_client(id);
                                        continue
                                    }
                                };
                            }

                        }
                        if event.is_readable() {
                            match throw_reader_at_writer(stream, &mut client.buf) {
                                Ok(()) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when reading: {e}");
                                    self.drop_client(id);
                                    continue
                                }
                            };
                            if !client.is_handshaking {
                                if !client.buf.has_read() { continue }
                                let story = client.buf.the_story_so_far();
    
                                match client.parser.parse(story) {
                                    Ok((Some(message), rest)) => {
                                        client.buf.consume(client.buf.data.len() - rest.len());
                                        client.sent_message = true;
    
                                        return Notification::SentMessage(id, message)
                                    },
                                    Ok((None, rest)) => {
                                        if rest == client.buf.data {
                                            println!("TLServer: request incomplete message at size {}", story.len())
                                        }
                                        else {
                                            let old_len = client.buf.data.len();
                                            let new_len = rest.len();
                                            client.buf.data.copy_within(old_len-new_len.., 0);
                                            client.buf.data.resize(new_len, 0);
                                            client.buf.read = new_len;
                                            client.buf.prev_read = new_len;
                                            println!("TlServer: read {} bytes for handshake", old_len - new_len)
                                        }
                                    },
                                    Err(e) => {
                                        println!("TLServer: dropped client on account of error when parsing request: {e}");
                                        self.drop_client(id);
                                        continue
                                    },
                                }
                            }

                        }
                        if !client.is_handshaking { continue }

                        // Application layer handshaking
                        loop {
                            match client.handshaker.handshake(client.buf.the_story_so_far()) {
                                Some((status, rest)) => {
                                    client.buf.consume(client.buf.data.len() - rest.len());
                                    match status {
                                        HandshakeStatus::Waiting => {},
                                        HandshakeStatus::Responding => {
                                            match client.handshaker.send_all(stream) {
                                                Ok(_) => continue,
                                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                                Err(e) => {
                                                    println!("HTTP_SERVER: dropped client on account of error when writing: {e}");
                                                    self.drop_client(id);
                                                }
                                            };
                                        },
                                        HandshakeStatus::Done => {
                                            println!("HTTP_SERVER: finished handshake with client {id}");
                                            client.is_handshaking = false
                                        },
                                    }
                                },
                                None => {
                                    self.drop_client(id);
                                }
                            }
                            break
                        }
                    }
                }
            }
        }
    }
    pub fn drop_client(&mut self, id: StreamId) {
        let client = self.clients.get_mut(&id).unwrap();
        
        let registry = self.poll.registry();
        registry.deregister(&mut client.stream).unwrap();
        
        if client.sent_message {
            self.queued_disconnects.push(id);
        }
        self.clients.remove(&id).unwrap();
    }
    pub fn send_to_client(&mut self, id: StreamId, message: impl Into<M>) {
        let client = self.clients.get_mut(&id).unwrap();
        client.messenger = message.into();
        match client.messenger.send_all(&mut client.stream) {
            Ok(_) => {},
            Err(e) if e.kind() == ErrorKind::WouldBlock => {},
            Err(e) => {
                println!("HTTP_SERVER: dropped client on account of error when writing: {e}");
                self.drop_client(id);
            }
        };
    }
    fn register(&mut self, client: TcpStream) -> StreamId {
        let registry = self.poll.registry();
        let mut stream = TLStream::new(client, self.config.clone());
        let mut id = fastrand::usize(..);
        while self.clients.contains_key(&id) {
            id = fastrand::usize(..);
        }
        let token = Token(id as usize);
        let interests = Interest::READABLE | Interest::WRITABLE;
        registry.register(&mut stream, token, interests).unwrap();

        let client = Client::new(id, stream);
        self.clients.insert(id, client);

        return id
    }
}

impl<M, P, T, E, H> Iterator for &mut Server_G<M, P, T, E, H>
where 
    M: SendTo + Default + From<Vec<u8>>,
    P: Parser<T, E> + Default,
    E: fmt::Display,
    H: Handshaker + Default,
{
    type Item = Notification<T>;
    fn next(&mut self) -> Option<Self::Item> {
        Some(self.serve())
    }
}

pub struct Client<M, P, T, E, H>
where 
    M: SendTo + Default,
    P: Parser<T, E> + Default,
    E: fmt::Display,
    H: Handshaker + Default,
{
    pub id: StreamId,
    pub stream: TLStream,
    pub buf: Buffer,
    pub messenger: M,
    pub parser: P,
    pub handshaker: H,
    pub is_handshaking: bool,
    pub sent_message: bool,
    t: PhantomData<T>,
    e: PhantomData<E>,
}

impl<M, P, T, E, H> Client<M, P, T, E, H>
where 
    M: SendTo + Default,
    P: Parser<T, E> + Default,
    E: fmt::Display,
    H: Handshaker + Default,
{
    fn new(id: StreamId, stream: TLStream) -> Self {
        Self {
            id,
            stream,
            buf: Buffer::with_capacity(4096), //TODO: maybe this should be less aligned?
            messenger: M::default(),
            parser: P::default(),
            handshaker: H::default(),
            is_handshaking: true,
            sent_message: false,
            t: PhantomData,
            e: PhantomData,
        }
    }
}