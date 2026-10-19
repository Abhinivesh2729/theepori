//! Blocking client for the tpd socket (CLI, tests, desktop worker threads).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tp_proto::{Event, Request, Response};

use crate::{Error, Result};

pub struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl Client {
    pub fn connect(socket: &Path, timeout: Duration) -> Result<Self> {
        let stream = UnixStream::connect(socket).map_err(|e| {
            Error::Daemon(format!(
                "cannot reach tpd at {} ({e}); start it with `tp daemon install` or `tp daemon run`",
                socket.display()
            ))
        })?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| Error::io(socket, e))?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|e| Error::io(socket, e))?;
        let reader = BufReader::new(stream.try_clone().map_err(|e| Error::io(socket, e))?);
        Ok(Self {
            writer: stream,
            reader,
            next_id: 1,
        })
    }

    pub fn call_raw(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = serde_json::to_vec(&Request {
            id,
            method: method.into(),
            params,
        })?;
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .map_err(|e| Error::Daemon(format!("send: {e}")))?;
        let resp: Response = serde_json::from_str(&self.read_line()?)?;
        if resp.id != id {
            return Err(Error::Daemon(format!(
                "response id {} for request {id}",
                resp.id
            )));
        }
        match (resp.result, resp.error) {
            (_, Some(e)) => Err(Error::Daemon(format!("{:?}: {}", e.code, e.message))),
            (Some(v), None) => Ok(v),
            (None, None) => Ok(Value::Null),
        }
    }

    pub fn call<T: DeserializeOwned>(&mut self, method: &str, params: Value) -> Result<T> {
        Ok(serde_json::from_value(self.call_raw(method, params)?)?)
    }

    /// Turns this connection into an event stream. Blocks per event; set a
    /// long timeout (or none) with [`Client::set_timeout`] first.
    pub fn subscribe(mut self) -> Result<impl Iterator<Item = Result<Event>>> {
        self.call_raw(tp_proto::method::SUBSCRIBE, Value::Null)?;
        Ok(std::iter::from_fn(move || match self.read_line() {
            Ok(l) => Some(serde_json::from_str(&l).map_err(Error::from)),
            Err(e) => Some(Err(e)),
        }))
    }

    pub fn set_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        self.writer
            .set_read_timeout(timeout)
            .map_err(|e| Error::Daemon(e.to_string()))
    }

    fn read_line(&mut self) -> Result<String> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| Error::Daemon(format!("receive: {e}")))?;
        if n == 0 {
            return Err(Error::Daemon("tpd closed the connection".into()));
        }
        Ok(line)
    }
}
