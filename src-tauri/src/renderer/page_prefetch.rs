//! Bounded background brick preparation. No GL calls are made by this worker.
use std::{collections::{HashMap,HashSet}, path::Path, sync::{Arc, atomic::{AtomicBool,Ordering},mpsc::{self,Receiver,SyncSender,TryRecvError,TrySendError}},thread::{self,JoinHandle}};
use super::paged_source::{Brick,PagedSource};
type Page=[u32;2];
const MAX_PENDING:usize=16;

struct Worker<T> {
    tx:Option<SyncSender<Page>>,rx:Option<Receiver<(Page,Result<T,String>)>>,
    pending:HashSet<Page>,ready:HashMap<Page,Result<T,String>>,
    cancel:Arc<AtomicBool>,thread:Option<JoinHandle<()>>,limit:usize,
}
impl<T:Send+'static> Worker<T> {
    fn new(limit:usize,mut read:impl FnMut(Page)->Result<T,String>+Send+'static)->Self {
        let limit=limit.clamp(1,MAX_PENDING);
        let (tx,requests)=mpsc::sync_channel::<Page>(limit);
        let (results,rx)=mpsc::sync_channel(limit);
        let cancel=Arc::new(AtomicBool::new(false));let stop=cancel.clone();
        let worker=thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let Ok(page)=requests.recv() else {break;};
                if stop.load(Ordering::Acquire) {break;}
                let value=read(page);
                if stop.load(Ordering::Acquire) || results.send((page,value)).is_err() {break;}
            }
        });
        Self {tx:Some(tx),rx:Some(rx),pending:HashSet::new(),ready:HashMap::new(),cancel,thread:Some(worker),limit}
    }
    fn try_request(&mut self,page:Page)->bool {
        if self.pending.contains(&page) || self.pending.len()>=self.limit {return false;}
        match self.tx.as_ref().map(|tx|tx.try_send(page)) {
            Some(Ok(()))=>{self.pending.insert(page);true},
            Some(Err(TrySendError::Full(_)|TrySendError::Disconnected(_)))|None=>false,
        }
    }
    fn receive(&mut self) {
        let Some(rx)=&self.rx else{return;};
        loop {match rx.try_recv() {
            Ok((p,result))=>{self.ready.insert(p,result);},
            Err(TryRecvError::Empty)=>break,
            Err(TryRecvError::Disconnected)=>{
                for page in &self.pending {self.ready.entry(*page).or_insert_with(||Err("页面预取线程已停止".into()));}
                break;
            }
        }}
    }
    fn try_take(&mut self,page:Page)->Option<Result<T,String>> {
        self.receive();let result=self.ready.remove(&page)?;self.pending.remove(&page);Some(result)
    }
    fn drain(&mut self)->Vec<(Page,Result<T,String>)> {
        self.receive();let results:Vec<_>=self.ready.drain().collect();
        for (page,_) in &results {self.pending.remove(page);}results
    }
}
impl<T> Drop for Worker<T> {
    fn drop(&mut self) {
        self.cancel.store(true,Ordering::Release);
        self.tx.take();self.rx.take();
        // An already-running source read completes; queued reads are discarded.
        // Dropping both channels also unblocks a receiver or result sender.
        if let Some(worker)=self.thread.take() {let _=worker.join();}
    }
}

pub(super) struct PagePrefetch {worker:Worker<Brick>}
impl PagePrefetch {
    /// A separate TIFF handle has a separate bounded decode cache. Other source
    /// formats could duplicate a whole decoded raster, so are not enabled here.
    pub(super) fn new(path:&Path,range:[f64;2],factor:u32,curve:f32,geo_halo:u32,
        resident:Option<Arc<Vec<f32>>>,decode_budget:usize)->Option<Self> {
        if !super::source_grid::is_tiff(path) {return None;}
        let path=path.to_owned();
        let mut source:Option<Result<PagedSource,String>>=None;
        Some(Self {worker:Worker::new(MAX_PENDING,move |page| {
            let source=source.get_or_insert_with(||{
                let mut s=PagedSource::open(&path,range,factor,curve,geo_halo,decode_budget)?;
                s.restore_resident(resident.clone());Ok(s)
            });
            match source {Ok(s)=>s.read_brick(page[0],page[1]),Err(e)=>Err(e.clone())}
        })})
    }
    pub(super) fn try_request(&mut self,page:Page)->bool {self.worker.try_request(page)}
    pub(super) fn try_take(&mut self,page:Page)->Option<Result<Brick,String>> {self.worker.try_take(page)}
    pub(super) fn drain(&mut self)->Vec<(Page,Result<Brick,String>)> {self.worker.drain()}
}


