//! Bounded CPU policy for page requests; waiting work never pins a GPU slot.
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy)]
struct Demand { first: u64, last: u64 }

pub(super) struct DemandScheduler {
    round: u64,
    starvation_rounds: u64,
    capacity: usize,
    pending: HashMap<i32, Demand>,
    recent: HashMap<i32, u64>,
}
impl DemandScheduler {
    pub(super) fn new(capacity: usize, starvation_rounds: u64) -> Self {
        Self { round: 0, starvation_rounds: starvation_rounds.max(1), capacity,
            pending: HashMap::new(), recent: HashMap::new() }
    }
    /// One call per feedback round. Counts are summed before ranking. Persistent
    /// bookkeeping is capped independently of the size of the virtual terrain.
    pub(super) fn rank(&mut self, requests: &[(i32,u32)], resident: &[i32]) -> Vec<i32> {
        self.round = self.round.saturating_add(1);
        let resident: HashSet<_> = resident.iter().copied().collect();
        for &page in &resident {
            if page < 0 {continue;}
            if !self.recent.contains_key(&page) && self.recent.len()>=self.capacity {
                if let Some(old)=self.recent.iter().min_by_key(|(p,r)|(**r,**p)).map(|(p,_)|*p) {
                    self.recent.remove(&old);
                }
            }
            if self.capacity>0 {self.recent.insert(page,self.round);}
        }
        let mut counts = BTreeMap::<i32,u64>::new();
        for &(page,count) in requests {
            if page >= 0 && count > 0 && !resident.contains(&page) {
                let sum = counts.entry(page).or_default();
                *sum = sum.saturating_add(count as u64);
            }
        }
        self.pending.retain(|page,_| counts.contains_key(page));
        let round = self.round;
        let mut ranked: Vec<_> = counts.into_iter().collect();
        ranked.sort_by(|(a,ac),(b,bc)| {
            let priority = |page: &i32,count:u64| {
                let old = self.pending.get(page);
                let age = old.map_or(1,|d| round.saturating_sub(d.first).saturating_add(1));
                let consecutive = old.is_some_and(|d| d.last.saturating_add(1)==round);
                // Starved requests precede even arbitrarily hot newcomers.
                (age >= self.starvation_rounds,
                 if age >= self.starvation_rounds { age } else { 0 },
                 count.saturating_mul(age),
                 self.recent.get(page).map_or(0,|last| self.starvation_rounds.saturating_sub(round.saturating_sub(*last))),
                 consecutive)
            };
            priority(b,*bc).cmp(&priority(a,*ac)).then_with(||a.cmp(b))
        });
        // Keep oldest waiters first. Every returned request remains eligible,
        // including requests beyond the bounded history capacity.
        for &(page,_) in &ranked {
            if let Some(d) = self.pending.get_mut(&page) { d.last=round; }
            else if self.pending.len()<self.capacity {
                self.pending.insert(page,Demand {first:round,last:round});
            }
        }
        ranked.into_iter().map(|(page,_)|page).collect()
    }
    pub(super) fn select_batch(&mut self, requests:&[(i32,u32)],resident:&[i32],limit:usize)->Vec<i32> {
        self.rank(requests,resident).into_iter().take(limit).collect()
    }
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) struct SlotToken { pub slot:usize, pub generation:u64, pub page:i32 }
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum Phase { Empty, Loading, Ready }
struct Slot {page:i32,generation:u64,phase:Phase,inflight:u32,last:u64}
/// Ownership model only: the caller must release execution references after its
/// GPU fence signals. Upload completion must precede commit and page-table use.
pub(super) struct SlotPool { slots:Vec<Slot>, clock:u64 }
impl SlotPool {
    pub(super) fn new(capacity:usize)->Self {
        Self { slots:(0..capacity).map(|_|Slot {page:-1,generation:0,phase:Phase::Empty,inflight:0,last:0}).collect(),clock:0 }
    }
    fn matches(slot:&Slot,t:SlotToken)->bool {slot.generation==t.generation && slot.page==t.page}
    pub(super) fn reserve(&mut self,page:i32)->Option<SlotToken> {
        if page<0 || self.slots.iter().any(|s|s.phase!=Phase::Empty && s.page==page) {return None;}
        let index=self.slots.iter().enumerate().filter(|(_,s)|s.inflight==0 && s.phase!=Phase::Loading)
            .min_by_key(|(_,s)|(s.phase!=Phase::Empty,s.last)).map(|(i,_)|i)?;
        self.clock=self.clock.saturating_add(1);
        let slot=&mut self.slots[index];
        // A wrapped generation must never make a stale token valid again.
        slot.generation=slot.generation.checked_add(1)?;
        slot.page=page;slot.phase=Phase::Loading;slot.last=self.clock;
        Some(SlotToken {slot:index,generation:slot.generation,page})
    }
    pub(super) fn commit(&mut self,t:SlotToken)->bool {
        let Some(s)=self.slots.get_mut(t.slot) else{return false;};
        if !Self::matches(s,t) || s.phase!=Phase::Loading {return false;}
        s.phase=Phase::Ready;true
    }
    pub(super) fn cancel(&mut self,t:SlotToken)->bool {
        let Some(s)=self.slots.get_mut(t.slot) else{return false;};
        if !Self::matches(s,t) || s.phase!=Phase::Loading {return false;}
        s.phase=Phase::Empty;true
    }
    pub(super) fn acquire(&mut self,page:i32)->Option<SlotToken> {
        let (index,s)=self.slots.iter_mut().enumerate().find(|(_,s)|s.page==page && s.phase==Phase::Ready)?;
        s.inflight=s.inflight.checked_add(1)?;
        self.clock=self.clock.saturating_add(1);s.last=self.clock;
        Some(SlotToken {slot:index,generation:s.generation,page})
    }
    pub(super) fn release(&mut self,t:SlotToken)->bool {
        let Some(s)=self.slots.get_mut(t.slot) else{return false;};
        if !Self::matches(s,t) || s.phase!=Phase::Ready || s.inflight==0 {return false;}
        s.inflight-=1;true
    }
    pub(super) fn resident_ids(&self)->Vec<i32> {
        self.slots.iter().filter(|s|s.phase==Phase::Ready).map(|s|s.page).collect()
    }
}


