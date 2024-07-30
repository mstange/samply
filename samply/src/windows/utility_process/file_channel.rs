//! This file contains an implementation of a bidirectional communication channel with
//! a remote process using files in a temporary directory.
//!
//! It was created for use case of having the root samply process control an elevated
//! helper process which collects ETW information. The elevated process is launched
//! with "runas". The elevated process is not a child process of the parent samply
//! process, so it doesn't inherit any handles for e.g. unnamed pipes.
//!
//! We could have probably used sockets for communication instead.
//!
//! The implementation in this file uses two text files and four lock files.
//! The lock files allow one side to block until the other side has written a new
//! message into the text file. They also allow one side to detect when the other
//! side has gone away prematurely - the file lock will be lifted but the text file
//! won't have any new content.
//!
//! The messages are written as length-delimited JSON. This is good enough for our
//! purposes.

use std::error::Error;
use std::fmt::Debug;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::path::Path;

use fs4::fs_std::FileExt;
use fs4::lock_contended_error;
use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Debug)]
struct FileMessageWriter<T> {
    file: std::fs::File,
    buf: Vec<u8>,
    _phantom: PhantomData<T>,
}

impl<T: Serialize> FileMessageWriter<T> {
    pub fn new(file: std::fs::File) -> Self {
        Self {
            file,
            buf: Vec::new(),
            _phantom: PhantomData,
        }
    }

    pub fn send(&mut self, msg: T) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.buf.clear();
        serde_json::to_writer(&mut self.buf, &msg)?;
        let len = u32::try_from(self.buf.len())?;
        self.file.write_all(&len.to_be_bytes())?;
        self.file.write_all(&self.buf)?;
        Ok(())
    }
}

#[derive(Debug)]
struct FileMessageReader<T> {
    file: std::fs::File,
    buf: Vec<u8>,
    _phantom: PhantomData<T>,
}

impl<T: DeserializeOwned> FileMessageReader<T> {
    pub fn new(file: std::fs::File) -> Self {
        Self {
            file,
            buf: Vec::new(),
            _phantom: PhantomData,
        }
    }

    pub fn recv(&mut self) -> Result<T, Box<dyn Error + Send + Sync>> {
        let mut len_bytes = [0; 4];
        self.file.read_exact(&mut len_bytes)?;
        let msg_len = u32::from_be_bytes(len_bytes);
        let msg_len = usize::try_from(msg_len).unwrap();
        self.buf.resize(msg_len, 0);
        self.file.read_exact(&mut self.buf)?;
        let msg_res = serde_json::from_slice(&self.buf);
        Ok(msg_res?)
    }
}

#[derive(Debug)]
pub struct Receiver<T> {
    reader: FileMessageReader<T>,
    /// Locked by the other side most of the time; only unlocked once a new message is available.
    current_lock: std::fs::File,
    /// Becomes current_lock once current_lock is unlocked.
    next_lock: std::fs::File,
}

impl<T: DeserializeOwned> Receiver<T> {
    pub fn open(path: &Path, create: bool) -> std::io::Result<Self> {
        let mut options = OpenOptions::new();
        options.create(create).write(create).read(true);
        let msgs_file = options.open(path)?;
        let current_lock = options.open(path.with_extension("lock1"))?;
        let next_lock = options.open(path.with_extension("lock2"))?;
        let reader = FileMessageReader::new(msgs_file);
        Ok(Self {
            reader,
            current_lock,
            next_lock,
        })
    }

    fn poll_until_other_side_exists(&self) -> std::io::Result<()> {
        // Poll until self.current_lock is locked by the other side.
        loop {
            match self.current_lock.try_lock_exclusive() {
                Ok(()) => {
                    // Not locked yet.
                    self.current_lock.unlock().unwrap();
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.raw_os_error() == lock_contended_error().raw_os_error() => {
                    // Success! The helper process now owns the file lock of self.current_lock.
                    break;
                }
                Err(e) => return Err(e),
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        Ok(())
    }

    pub fn recv_blocking(&mut self) -> Result<T, Box<dyn Error + Send + Sync>> {
        self.current_lock.lock_exclusive()?; // block until init message is written
        let msg = self.reader.recv()?;
        self.current_lock.unlock()?;
        std::mem::swap(&mut self.current_lock, &mut self.next_lock);
        Ok(msg)
    }
}

#[derive(Debug)]
pub struct Sender<T> {
    writer: FileMessageWriter<T>,
    /// Always locked by us.
    current_lock: std::fs::File,
    /// Becomes current_lock once a message has been written, just before we unlock the old current_lock.
    next_lock: std::fs::File,
}

impl<T: Serialize> Sender<T> {
    pub fn open(path: &Path, create: bool) -> std::io::Result<Self> {
        let mut options = OpenOptions::new();
        options.create(create).write(true);
        let msgs_file = options.open(path)?;
        let current_lock = options.open(path.with_extension("lock1"))?;
        let next_lock = options.open(path.with_extension("lock2"))?;
        current_lock.lock_exclusive()?;
        let writer = FileMessageWriter::new(msgs_file);
        Ok(Self {
            writer,
            current_lock,
            next_lock,
        })
    }

    pub fn send(&mut self, msg: T) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.writer.send(msg)?;
        self.next_lock.lock_exclusive()?; // ready next lock
        std::mem::swap(&mut self.current_lock, &mut self.next_lock);
        self.next_lock.unlock()?; // indicate that reply has been written
        Ok(())
    }
}

pub struct BidiChannelCreator<ParentToChildMsg, ChildToParentMsg> {
    receiver: Receiver<ChildToParentMsg>,
    sender: Sender<ParentToChildMsg>,
}

impl<ParentToChildMsg, ChildToParentMsg> BidiChannelCreator<ParentToChildMsg, ChildToParentMsg>
where
    ParentToChildMsg: Serialize + DeserializeOwned,
    ChildToParentMsg: Serialize + DeserializeOwned,
{
    pub fn create_in_parent(ipc_dir: &Path) -> std::io::Result<Self> {
        let msgs_to_child_path = ipc_dir.join("msgs_to_child.txt");
        let sender = Sender::open(&msgs_to_child_path, true)?;

        let msgs_to_parent_path = ipc_dir.join("msgs_to_parent.txt");
        let receiver = Receiver::open(&msgs_to_parent_path, true)?;

        Ok(Self { receiver, sender })
    }

    pub fn open_in_child(
        ipc_dir: &Path,
    ) -> std::io::Result<(Receiver<ParentToChildMsg>, Sender<ChildToParentMsg>)> {
        let msgs_to_child_path = ipc_dir.join("msgs_to_child.txt");
        let receiver = Receiver::open(&msgs_to_child_path, false)?;

        let msgs_to_parent_path = ipc_dir.join("msgs_to_parent.txt");
        let sender = Sender::open(&msgs_to_parent_path, false)?;

        Ok((receiver, sender))
    }

    pub fn wait_for_child_to_connect(
        self,
    ) -> std::io::Result<(Receiver<ChildToParentMsg>, Sender<ParentToChildMsg>)> {
        self.receiver.poll_until_other_side_exists()?;

        Ok((self.receiver, self.sender))
    }
}
