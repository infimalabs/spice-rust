//! Page-local singleton scheduling. Cancellation drops subscribers, not running JS.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use gloo_timers::callback::Timeout;
use yew::{AttrValue, Callback};

use super::Theme;
use super::frame::Frame;

const SOURCE_UNITS: usize = 50_000;
const QUEUE_JOBS: usize = 16;
const QUEUE_BYTES: usize = 1024 * 1024;
const SUBSCRIBERS: usize = 64;
const CACHE_ENTRIES: usize = 32;
const CACHE_BYTES: usize = 16 * 1024 * 1024;
const COALESCE_MILLIS: u32 = 80;
const RESPONSE_MILLIS: u32 = 30_000;

pub(super) type Outcome = Result<AttrValue, AttrValue>;
pub(super) type Shared = Rc<RefCell<Service>>;

#[derive(Clone, PartialEq)]
pub(super) struct Key {
    pub(super) source: AttrValue,
    pub(super) theme: Theme,
    pub(super) generation: AttrValue,
}

pub(super) struct Subscriber {
    pub(super) id: u32,
    pub(super) host: web_sys::Element,
    pub(super) revision: AttrValue,
    pub(super) callback: Callback<Outcome>,
}

impl Subscriber {
    fn attached(&self) -> bool {
        self.host.is_connected()
            && self.host.get_attribute("data-mermaid-revision").as_deref()
                == Some(self.revision.as_str())
    }

    fn deliver(self, outcome: Outcome) {
        if self.attached() {
            self.callback.emit(outcome);
        }
    }
}

pub(super) struct Job {
    pub(super) id: u32,
    pub(super) key: Key,
    pub(super) subscribers: Vec<Subscriber>,
    pub(super) decoding: Option<super::frame::DecodedImage>,
}

pub(super) struct Service {
    pub(super) generation: AttrValue,
    pub(super) channel: String,
    pub(super) frame: Option<Frame>,
    pub(super) active: Option<Job>,
    queue: VecDeque<Job>,
    cache: VecDeque<(Key, AttrValue)>,
    cache_bytes: usize,
    serial: u32,
    dispatch: Option<Timeout>,
    deadline: Option<Timeout>,
}

pub(super) struct Lease {
    service: Weak<RefCell<Service>>,
    id: u32,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(service) = self.service.upgrade() {
            service.borrow_mut().cancel(self.id);
        }
    }
}

impl Service {
    pub(super) fn new(generation: wire::Digest) -> Result<Shared, String> {
        let channel = super::super::random_uuid()?.to_string();
        Ok(Rc::new(RefCell::new(Self {
            generation: generation.to_string().into(),
            channel,
            frame: None,
            active: None,
            queue: VecDeque::new(),
            cache: VecDeque::new(),
            cache_bytes: 0,
            serial: 0,
            dispatch: None,
            deadline: None,
        })))
    }

    pub(super) fn request(
        shared: &Shared,
        source: AttrValue,
        theme: Theme,
        host: web_sys::Element,
        revision: AttrValue,
        callback: Callback<Outcome>,
    ) -> Result<Lease, AttrValue> {
        if source.encode_utf16().take(SOURCE_UNITS + 1).count() > SOURCE_UNITS {
            return Err("Diagram exceeds the 50,000 UTF-16 unit source limit".into());
        }
        let mut service = shared.borrow_mut();
        let id = service
            .serial
            .checked_add(1)
            .ok_or("Renderer request limit reached; reload Canvas")?;
        service.serial = id;
        let key = Key {
            source,
            theme,
            generation: service.generation.clone(),
        };
        let subscriber = Subscriber {
            id,
            host,
            revision,
            callback,
        };
        // Rejection must not drop a lease while the mutable service borrow is held.
        let lease = || Lease {
            service: Rc::downgrade(shared),
            id,
        };
        if let Some(index) = service.cache.iter().position(|(cached, _)| *cached == key) {
            let entry = service.cache.remove(index).expect("located cache entry");
            let image = entry.1.clone();
            service.cache.push_back(entry);
            drop(service);
            subscriber.deliver(Ok(image));
            return Ok(lease());
        }
        service.prune();
        let subscribers = service
            .queue
            .iter()
            .chain(service.active.iter())
            .map(|job| job.subscribers.len())
            .sum::<usize>();
        if subscribers >= SUBSCRIBERS {
            return Err(
                "Diagram renderer has 64 pending subscribers; retry when it is idle".into(),
            );
        }
        if let Some(job) = service.active.as_mut().filter(|job| job.key == key) {
            job.subscribers.push(subscriber);
        } else if let Some(job) = service.queue.iter_mut().find(|job| job.key == key) {
            job.subscribers.push(subscriber);
        } else {
            let bytes = service
                .queue
                .iter()
                .map(|job| job.key.source.len())
                .sum::<usize>();
            if service.queue.len() >= QUEUE_JOBS || bytes + key.source.len() > QUEUE_BYTES {
                return Err("Diagram render queue is full; retry when it is idle".into());
            }
            service.queue.push_back(Job {
                id,
                key,
                subscribers: vec![subscriber],
                decoding: None,
            });
        }
        drop(service);
        Self::schedule(shared);
        Ok(lease())
    }

    fn prune(&mut self) {
        for job in self.queue.iter_mut().chain(self.active.iter_mut()) {
            job.subscribers.retain(Subscriber::attached);
        }
        self.queue.retain(|job| !job.subscribers.is_empty());
    }

    fn cancel(&mut self, id: u32) {
        for job in self.queue.iter_mut().chain(self.active.iter_mut()) {
            job.subscribers.retain(|subscriber| subscriber.id != id);
        }
        self.queue.retain(|job| !job.subscribers.is_empty());
    }

    pub(super) fn schedule(shared: &Shared) {
        let mut service = shared.borrow_mut();
        service.prune();
        if service.queue.is_empty() || service.active.is_some() || service.dispatch.is_some() {
            return;
        }
        let weak = Rc::downgrade(shared);
        service.dispatch = Some(Timeout::new(COALESCE_MILLIS, move || {
            if let Some(shared) = weak.upgrade() {
                Self::pump(&shared);
            }
        }));
    }

    fn pump(shared: &Shared) {
        let mut service = shared.borrow_mut();
        service.dispatch = None;
        service.prune();
        if service.queue.is_empty() || service.active.is_some() {
            return;
        }
        if service.frame.is_none() {
            match Frame::open(shared, &service.generation) {
                Ok(frame) => {
                    service.frame = Some(frame);
                    service.arm_deadline(shared);
                }
                Err(_) => {
                    drop(service);
                    Self::close(shared, "Could not load the diagram renderer");
                }
            }
            return;
        }
        if !service.frame.as_ref().is_some_and(|frame| frame.ready) {
            return;
        }
        service.active = service.queue.pop_front();
        let result = service.frame.as_ref().expect("loaded renderer").send(
            service.active.as_ref().expect("queued render"),
            &service.channel,
        );
        if result.is_err() {
            drop(service);
            Self::close(shared, "Could not send the diagram to its renderer");
            return;
        }
        service.arm_deadline(shared);
    }

    fn arm_deadline(&mut self, shared: &Shared) {
        let weak = Rc::downgrade(shared);
        // This is a response deadline, not a CPU interrupt. An iframe can block
        // this event loop; recovery runs only once the browser can execute it.
        self.deadline = Some(Timeout::new(RESPONSE_MILLIS, move || {
            if let Some(shared) = weak.upgrade() {
                Self::close(
                    &shared,
                    "Diagram renderer did not respond; retry or reload Canvas",
                );
            }
        }));
    }

    pub(super) fn ready(shared: &Shared) {
        let mut service = shared.borrow_mut();
        if let Some(frame) = &mut service.frame {
            if frame.ready {
                return;
            }
            frame.ready = true;
            service.deadline = None;
        }
        drop(service);
        Self::schedule(shared);
    }

    pub(super) fn has_subscribers(job: &Job) -> bool {
        job.subscribers.iter().any(Subscriber::attached)
    }

    pub(super) fn finish(shared: &Shared, id: u32, outcome: Outcome) {
        let mut service = shared.borrow_mut();
        if !service.active.as_ref().is_some_and(|job| job.id == id) {
            return;
        }
        service.deadline = None;
        let job = service.active.take().expect("matched active render");
        if Self::has_subscribers(&job)
            && let Ok(image) = &outcome
        {
            service.cache(job.key, image.clone());
        }
        drop(service);
        for subscriber in job.subscribers {
            subscriber.deliver(outcome.clone());
        }
        Self::schedule(shared);
    }

    fn cache(&mut self, key: Key, image: AttrValue) {
        let bytes = key.source.len() + image.len();
        while self.cache.len() >= CACHE_ENTRIES || self.cache_bytes + bytes > CACHE_BYTES {
            let Some((old_key, old_image)) = self.cache.pop_front() else {
                return;
            };
            self.cache_bytes -= old_key.source.len() + old_image.len();
        }
        self.cache_bytes += bytes;
        self.cache.push_back((key, image));
    }

    pub(super) fn close(shared: &Shared, reason: &'static str) {
        let mut service = shared.borrow_mut();
        service.dispatch = None;
        service.deadline = None;
        service.frame = None;
        service.cache.clear();
        service.cache_bytes = 0;
        let active = service.active.take();
        let queue = std::mem::take(&mut service.queue);
        drop(service);
        let error = AttrValue::from(reason);
        for job in active.into_iter().chain(queue) {
            for subscriber in job.subscribers {
                subscriber.deliver(Err(error.clone()));
            }
        }
    }
}
