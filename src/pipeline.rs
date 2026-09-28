//! Bounded grouped-write front buffer plus one storage worker.
//! Only the worker owns Session/the device. Mutations drain earlier writes;
//! queries overlay volatile inputs. fsync/close wait and report storage errors.
use crate::{buffered::{Session, WritePolicy}, journal};
use anyhow::{ensure, Context, Result};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::JoinHandle;

type Job = Box<dyn FnOnce(&mut Session) + Send>;
struct Write {
    offset: u64,
    bytes: Vec<u8>,
    sha256: String,
}
pub struct Pipeline {
    sender: Option<SyncSender<Job>>,
    worker: Option<JoinHandle<()>>,
    active: Option<Receiver<Result<()>>>,
    pending: Vec<Write>,
    pending_bytes: u64,
    cached: Option<(String, apfs::Attr)>,
    group_bytes: u64,
    policy: WritePolicy,
    failed: Option<String>,
    #[cfg(feature = "fault-injection")]
    fail_next_batch: bool,
}
impl Pipeline {
    pub fn new(mut session: Session) -> Result<Self> {
        let policy = session.write_policy();
        let group_bytes = session.group_bytes();
        let (sender, receive) = mpsc::sync_channel::<Job>(1);
        let worker = std::thread::Builder::new().name("lapfs-storage".into()).spawn(move || {
            while let Ok(job) = receive.recv() { job(&mut session); }
        })?;
        Ok(Self {
            sender: Some(sender), worker: Some(worker), active: None,
            pending: Vec::new(), pending_bytes: 0, cached: None,
            group_bytes, policy, failed: None,
            #[cfg(feature = "fault-injection")]
            fail_next_batch: false,
        })
    }
    fn ready(&self) -> Result<()> {
        ensure!(self.failed.is_none(), "Storage pipeline failed; preserve recovery files: {}", self.failed.as_deref().unwrap_or(""));
        ensure!(self.sender.is_some() && self.worker.as_ref().is_some_and(|w| !w.is_finished()), "Storage worker terminated; preserve recovery files");
        Ok(())
    }
    fn completion(&mut self, result: Result<()>) -> Result<()> {
        if let Err(e) = result {
            self.failed = Some(format!("{e:#}"));
            return Err(e);
        }
        if self.pending.is_empty() { self.cached = None; }
        Ok(())
    }
    fn poll(&mut self) -> Result<()> {
        self.ready()?;
        if let Some(rx) = &self.active {
            match rx.try_recv() {
                Ok(result) => { self.active = None; self.completion(result)?; }
                Err(TryRecvError::Empty) => (),
                Err(TryRecvError::Disconnected) => {
                    self.active = None;
                    self.completion(Err(anyhow::anyhow!("Storage worker terminated")))?;
                }
            }
        }
        Ok(())
    }
    fn join(&mut self) -> Result<()> {
        self.ready()?;
        if let Some(rx) = self.active.take() {
            let result = rx.recv().unwrap_or_else(|_| Err(anyhow::anyhow!("Storage worker terminated")));
            self.completion(result)?;
        }
        Ok(())
    }
    fn call<T: Send + 'static>(&mut self, f: impl FnOnce(&mut Session) -> Result<T> + Send + 'static) -> Result<T> {
        self.ready()?;
        self.join()?;
        let (send, recv) = mpsc::channel();
        self.sender.as_ref().context("Storage worker closed")?.send(Box::new(move |session| {
            let _ = send.send(f(session));
        })).map_err(|_| anyhow::anyhow!("Storage worker terminated"))?;
        match recv.recv() {
            Ok(result) => result,
            Err(_) => {
                self.failed = Some("Storage worker terminated".into());
                Err(anyhow::anyhow!("Storage worker terminated; preserve recovery files"))
            }
        }
    }
    fn dispatch(&mut self) -> Result<()> {
        if self.pending.is_empty() { return self.ready(); }
        // At most one committing batch plus this input batch; no unbounded queue.
        self.join()?;
        let path = self.cached.as_ref().context("Missing pending file identity")?.0.clone();
        let batch = std::mem::take(&mut self.pending);
        self.pending_bytes = 0;
        let (send, recv) = mpsc::channel();
        #[cfg(feature = "fault-injection")]
        let fail = std::mem::take(&mut self.fail_next_batch);
        self.sender.as_ref().context("Storage worker closed")?.send(Box::new(move |session| {
            let result = (|| {
                journal::fault_point("pipeline-batch-start");
                #[cfg(feature = "fault-injection")]
                ensure!(!fail, "injected pipeline storage failure");
                for write in batch {
                    ensure!(journal::hash(&write.bytes) == write.sha256, "Pipeline input checksum mismatch");
                    session.write(&path, write.offset, &write.bytes)?;
                }
                session.flush()?;
                journal::fault_point("pipeline-batch-complete");
                Ok(())
            })();
            let _ = send.send(result);
        })).map_err(|_| anyhow::anyhow!("Storage worker terminated"))?;
        self.active = Some(recv);
        Ok(())
    }
    fn settle(&mut self) -> Result<()> {
        self.dispatch()?;
        self.join()?;
        self.cached = None;
        Ok(())
    }
    pub fn writable(&mut self, path: &str) -> Result<()> {
        self.poll()?;
        if self.cached.as_ref().is_some_and(|(p, _)| p == path) { return Ok(()); }
        self.settle()?;
        let p = path.to_owned();
        let attr = self.call(move |s| { s.writable(&p)?; s.attr(&p) })?;
        self.cached = Some((path.to_owned(), attr));
        Ok(())
    }
    pub fn attr(&mut self, path: &str) -> Result<apfs::Attr> {
        self.poll()?;
        if let Some((p, attr)) = &self.cached {
            if p == path { return Ok(*attr); }
        }
        let p = path.to_owned(); self.call(move |s| s.attr(&p))
    }
    pub fn write(&mut self, path: &str, offset: u64, data: &[u8]) -> Result<()> {
        self.poll()?;
        if self.policy == WritePolicy::Durable {
            self.cached = None;
            let p = path.to_owned(); let data = data.to_vec();
            return self.call(move |s| s.write(&p, offset, &data));
        }
        self.writable(path)?;
        let size = self.cached.as_ref().unwrap().1.size;
        if offset > size { return Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP).into()); }
        ensure!(offset.checked_add(data.len() as u64).is_some_and(|n| n <= i64::MAX as u64), "Write offset overflow");
        if data.is_empty() { return Ok(()); }
        if data.len() as u64 > self.group_bytes { return Err(std::io::Error::from_raw_os_error(libc::EFBIG).into()); }
        if self.pending_bytes + data.len() as u64 > self.group_bytes || self.pending.len() >= 8191 {
            self.dispatch()?;
            // dispatch may consume a completion and invalidate an empty cache.
            self.writable(path)?;
        }
        self.pending.try_reserve(1).map_err(|_| std::io::Error::from_raw_os_error(libc::ENOMEM))?;
        self.pending.push(Write { offset, bytes: data.to_vec(), sha256: journal::hash(data) });
        self.pending_bytes += data.len() as u64;
        let attr = &mut self.cached.as_mut().unwrap().1;
        attr.size = attr.size.max(offset + data.len() as u64);
        journal::fault_point("pipeline-input-accepted");
        if self.pending_bytes >= self.group_bytes || self.pending.len() >= 8191 { self.dispatch()?; }
        Ok(())
    }
    pub fn flush(&mut self) -> Result<()> { self.settle()?; self.call(|s| s.flush()) }
    pub fn close(&mut self) -> Result<()> { self.settle()?; self.call(|s| s.close()) }
    pub fn read(&mut self, path:&str, offset:u64, size:usize) -> Result<Vec<u8>> {
        // Join only the already submitted batch. Observers must not turn the
        // next volatile input buffer into many tiny durable transactions.
        self.poll()?;
        let logical_size = self.cached.as_ref().filter(|(p,_)| p==path && !self.pending.is_empty()).map(|(_,a)| a.size);
        let Some(logical_size) = logical_size else {
            let p=path.to_owned(); return self.call(move |s| s.read(&p,offset,size));
        };
        if offset >= logical_size || size == 0 { return Ok(Vec::new()); }
        let count = (logical_size-offset).min(size as u64) as usize;
        let p=path.to_owned(); let mut bytes=self.call(move |s| s.read(&p,offset,count))?;
        bytes.resize(count,0);
        if self.cached.as_ref().is_some_and(|(p,_)| p==path) {
            for write in &self.pending {
                let start=offset.max(write.offset);
                let end=(offset+count as u64).min(write.offset+write.bytes.len() as u64);
                if start<end {
                    if journal::hash(&write.bytes) != write.sha256 {
                        self.failed=Some("Pipeline input checksum mismatch".into());
                        anyhow::bail!("Pipeline input checksum mismatch; preserve recovery files");
                    }
                    bytes[(start-offset) as usize..(end-offset) as usize].copy_from_slice(
                        &write.bytes[(start-write.offset) as usize..(end-write.offset) as usize]);
                }
            }
        }
        Ok(bytes)
    }
    pub fn list(&mut self, path:&str) -> Result<Vec<apfs_core::catalog::DirEntry>> {
        let p=path.to_owned(); self.call(move |s| s.list(&p))
    }
    pub fn readlink(&mut self, path:&str) -> Result<Vec<u8>> {
        let p=path.to_owned(); self.call(move |s| s.readlink(&p))
    }
    pub fn space(&mut self) -> Result<(u64,u64)> {
        let (total,free)=self.call(|s| s.space())?;
        Ok((total,free.saturating_sub(self.pending_bytes.div_ceil(4096))))
    }
    pub fn create(&mut self,path:&str)->Result<()> {self.settle()?;let p=path.to_owned();self.call(move |s|s.create(&p))}
    pub fn mkdir(&mut self,path:&str)->Result<()> {self.settle()?;let p=path.to_owned();self.call(move |s|s.mkdir(&p))}
    pub fn unlink(&mut self,path:&str)->Result<()> {self.settle()?;let p=path.to_owned();self.call(move |s|s.unlink(&p))}
    pub fn rmdir(&mut self,path:&str)->Result<()> {self.settle()?;let p=path.to_owned();self.call(move |s|s.rmdir(&p))}
    pub fn truncate(&mut self,path:&str,size:u64)->Result<()> {self.settle()?;let p=path.to_owned();self.call(move |s|s.truncate(&p,size))}
    pub fn rename(&mut self,path:&str,destination:&str,replace:bool)->Result<()> {self.settle()?;let p=path.to_owned();let d=destination.to_owned();self.call(move |s|s.rename(&p,&d,replace))}
    pub fn symlink(&mut self,path:&str,target:&[u8])->Result<()> {self.settle()?;let p=path.to_owned();let t=target.to_vec();self.call(move |s|s.symlink(&p,&t))}
    pub fn set_attrs(&mut self,path:&str,mode:Option<u16>,atime:Option<u64>,mtime:Option<u64>)->Result<()> {self.settle()?;let p=path.to_owned();self.call(move |s|s.set_attrs(&p,mode,atime,mtime))}

    #[cfg(feature = "fault-injection")]
    pub fn test_pause_worker(&mut self) -> Result<mpsc::Sender<()>> {
        self.ready()?;
        ensure!(self.pending.is_empty() && self.active.is_none(), "Test gate requires idle pipeline");
        let (entered, wait_entered)=mpsc::channel(); let (release, wait_release)=mpsc::channel();
        self.sender.as_ref().unwrap().send(Box::new(move |_| {let _=entered.send(());let _=wait_release.recv();})).map_err(|_|anyhow::anyhow!("Worker stopped"))?;
        wait_entered.recv()?; Ok(release)
    }
    #[cfg(feature = "fault-injection")]
    pub fn test_panic_worker(&mut self) -> Result<()> {
        self.settle()?;
        self.call(|_| panic!("injected storage worker panic"))
    }
    #[cfg(feature = "fault-injection")]
    pub fn test_corrupt_pending(&mut self) { self.pending.last_mut().unwrap().bytes[0]^=1; }
    #[cfg(feature = "fault-injection")]
    pub fn test_fail_next_batch(&mut self) { self.fail_next_batch=true; }
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        // No detached writer may retain the device after the mount object dies.
        self.sender.take();
        if let Some(worker)=self.worker.take() { let _=worker.join(); }
    }
}
