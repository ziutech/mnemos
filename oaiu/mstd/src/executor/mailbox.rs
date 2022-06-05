// So the goal of the mailbox is basically a Request/Response server,
// with some additional messages sent unsolicited
//
// In an ideal form, it looks like this:
//
// 1. The userspace submits a message to be sent
// 2. Once there is room in the ring, it is serialized
// 3. The userspace waits on the response to come back
// 4. The response is deserialized, and the caller is given the response
// 5. The user reacts to the response
//
// So, we have a finite amount of resources, and there will need to be
// SOME kind of backpressure mechanism somewhere.
//
// This could be:
//
// ## Submission backpressure
//
// * The mailbox gives back a future when the user asks to submit a message
// * The mailbox readies the future when it has room in the "response map"
//   AND there is room in the ring to serialize the message
//     * TODO: How to "wake" the pending slots? Do we do a "jailbreak"
//       wake all? Or just wake the next N items based on available slots?
// * The mailbox exchanges the "send" future with a "receive" future
// * Once the response comes in, the task/future is retrieved from the
//     "response map", and awoken
// * The task "picks up" its message, and frees the space in the response map
//
// Downsides:
//
// A lot of small, slow responses could cause large and/or fast responses to be
// blocked on a pending response slot. Ideally, you could spam messages into
// the outgoing queue immediately (allowing them to be processed), but you'd need
// SOME way to process the response messages, and if we get back a response before
// the request has made it into the "response map", it'll be a problem.

use core::{sync::atomic::{AtomicBool, Ordering, AtomicU32}, mem::MaybeUninit, cell::UnsafeCell, future::Future, pin::Pin, task::{Context, Poll}};

use maitake::wait::WaitQueue;
use heapless::LinearMap;
use abi::{syscall::{request::SysCallRequest, success::SysCallSuccess}, bbqueue_ipc::framed::{FrameProducer, FrameConsumer}};

use crate::utils::ArfCell;

pub static MAILBOX: MailBox = MailBox::new();

// TODO: There's a LOT of mutexes going on here.
pub struct MailBox {
    nonce: AtomicU32,
    inhibit_send: AtomicBool,
    send_wait: WaitQueue,
    recv_wait: WaitQueue,
    rings: OnceRings,
    received: ArfCell<LinearMap<u32, Result<SysCallSuccess, ()>, 32>>
}

impl MailBox {
    pub const fn new() -> Self {
        Self {
            nonce: AtomicU32::new(0),
            inhibit_send: AtomicBool::new(false),
            send_wait: WaitQueue::new(),
            recv_wait: WaitQueue::new(),
            rings: OnceRings::new(),
            received: ArfCell::new(LinearMap::new()),
        }
    }

    pub fn set_rings(&self, rings: Rings) {
        self.rings.set(rings);
    }

    pub fn poll(&self) {
        let rings = self.rings.get();
        let mut recv = self.received.borrow_mut().unwrap();

        let mut any = false;

        'process: while recv.len() < recv.capacity() {
            match rings.k2u.read() {
                Some(msg) => {

                    assert!(msg.len() >= 4);
                    let (nonce, msgb) = msg.split_at(4);
                    let mut nonce_b = [0u8; 4];
                    nonce_b.copy_from_slice(nonce);
                    let nonce = u32::from_le_bytes(nonce_b);

                    match postcard::from_bytes::<Result<SysCallSuccess, ()>>(msgb) {
                        Ok(dec_msg) => {
                            recv.insert(nonce, dec_msg).ok();
                            any = true;
                        },
                        Err(_) => {
                            // todo: print something?
                        },
                    }

                    msg.release();
                },
                None => {
                    // All done!
                    break 'process;
                },
            }
        }

        if any {
            self.recv_wait.wake_all();
        }

        if self.inhibit_send.load(Ordering::Acquire) && rings.u2k.grant(128).is_ok() {
            self.inhibit_send.store(false, Ordering::Release);
            self.send_wait.wake_all();
        }
    }

    pub async fn send(&'static self, msg: SysCallRequest) -> Result<SysCallSuccess, ()> {
        let nonce = self.nonce.fetch_add(1, Ordering::AcqRel);
        let rings = self.rings.get();

        // Wait for a successful send
        loop {
            if !self.inhibit_send.load(Ordering::Acquire) {
                if let Ok(mut wgr) = rings.u2k.grant(128) { // TODO: Max Size
                    let (num, rest) = wgr.split_at_mut(4);
                    num.copy_from_slice(&nonce.to_le_bytes());
                    let used = postcard::to_slice(&msg, rest).map_err(drop)?.len();
                    wgr.commit(used + 4);
                    break;
                } else {
                    // Inhibit further sending until there is room, in order to prevent
                    // starving waiters
                    self.inhibit_send.store(true, Ordering::Release);
                }
            }
            self.send_wait
                .wait()
                .await
                .map_err(drop)?;
        }

        // Wait for successful receive
        loop {
            // Wait first, the message won't already be there (unless we got REALLY lucky)
            self.recv_wait
                .wait()
                .await
                .map_err(drop)?;

            if let Ok(mut rxg) = self.received.borrow_mut() {
                if let Some(rx) = rxg.remove(&nonce) {
                    return rx;
                }
            }
        }
    }
}

unsafe impl Sync for OnceRings { }

struct OnceRings {
    set: AtomicBool,
    queues: UnsafeCell<MaybeUninit<Rings>>,
}

impl OnceRings {
    const fn new() -> Self {
        Self {
            set: AtomicBool::new(false),
            queues: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    fn set(&self, rings: Rings) {
        unsafe {
            self.queues.get().cast::<Rings>().write(rings);
            let old = self.set.swap(true, Ordering::SeqCst);
            assert!(!old);
        }
    }

    fn get(&self) -> &Rings {
        assert!(self.set.load(Ordering::Relaxed));
        unsafe {
            &*self.queues.get().cast::<Rings>()
        }
    }
}

pub struct Rings {
    pub u2k: FrameProducer<'static>,
    pub k2u: FrameConsumer<'static>,
}

// impl Ma
