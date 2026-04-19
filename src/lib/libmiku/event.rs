// Event/callback system
// Simple publish-subscribe event dispatcher
// Register handlers by event ID, then fire events
// Useful for decoupling components in applications
// Thread-safe via SpinLock :)

use crate::sync::SpinLock;

const MAX_HANDLERS: usize = 64;

type EventHandler = extern "C" fn(u32, *mut u8, *mut u8);

#[derive(Copy, Clone)]
struct HandlerEntry {
    event_id: u32,
    handler: Option<EventHandler>,
    ctx: *mut u8,
    active: bool,
    once: bool,
}

// safe Send impl for handler entries
// ctx pointers are managed by the caller
unsafe impl Send for HandlerEntry {}

const EMPTY_HANDLER: HandlerEntry = HandlerEntry {
    event_id: 0,
    handler: None,
    ctx: core::ptr::null_mut(),
    active: false,
    once: false,
};

struct EventStore {
    handlers: [HandlerEntry; MAX_HANDLERS],
    high_water: usize,
}

static EVENTS: SpinLock<EventStore> = SpinLock::new(EventStore {
    handlers: [EMPTY_HANDLER; MAX_HANDLERS],
    high_water: 0,
});

// Register event handler
// Returns handler index, or -1 on failure
#[no_mangle]
pub extern "C" fn miku_event_on(
    event_id: u32,
    handler: EventHandler,
    ctx: *mut u8,
) -> i32 {
    let mut store = EVENTS.lock();
    for i in 0..MAX_HANDLERS {
        if !store.handlers[i].active {
            store.handlers[i] = HandlerEntry {
                event_id,
                handler: Some(handler),
                ctx,
                active: true,
                once: false,
            };
            if i >= store.high_water {
                store.high_water = i + 1;
            }
            return i as i32;
        }
    }
    -1
}

// remove handler by index
#[no_mangle]
pub extern "C" fn miku_event_off(index: i32) {
    if index < 0 || index as usize >= MAX_HANDLERS { return; }
    let mut store = EVENTS.lock();
    store.handlers[index as usize].active = false;
    store.handlers[index as usize].handler = None;
}

// fire event calls all handlers registered for event_id
// Handles one-shot handlers automatically
// Returns number of handlers called.
#[no_mangle]
pub extern "C" fn miku_event_emit(event_id: u32, data: *mut u8) -> u32 {
    // collect matching handlers under lock, then call outside
    let mut to_call: [core::mem::MaybeUninit<(EventHandler, *mut u8)>; MAX_HANDLERS] =
        unsafe { core::mem::MaybeUninit::uninit().assume_init() };
    let mut call_count = 0usize;

    {
        let mut store = EVENTS.lock();
        for i in 0..store.high_water {
            let h = &store.handlers[i];
            if h.active && h.event_id == event_id {
                if let Some(handler) = h.handler {
                    to_call[call_count] = core::mem::MaybeUninit::new((handler, h.ctx));
                    call_count += 1;
                }
                if h.once {
                    store.handlers[i].active = false;
                    store.handlers[i].handler = None;
                }
            }
        }
    }

    // call handlers without holding the lock
    for i in 0..call_count {
        let (handler, ctx) = unsafe { to_call[i].assume_init() };
        handler(event_id, data, ctx);
    }

    call_count as u32
}

// check if event has any handlers
#[no_mangle]
pub extern "C" fn miku_event_has_listeners(event_id: u32) -> bool {
    let store = EVENTS.lock();
    for i in 0..store.high_water {
        if store.handlers[i].active && store.handlers[i].event_id == event_id {
            return true;
        }
    }
    false
}

// count handlers for event
#[no_mangle]
pub extern "C" fn miku_event_count(event_id: u32) -> u32 {
    let mut n = 0u32;
    let store = EVENTS.lock();
    for i in 0..store.high_water {
        if store.handlers[i].active && store.handlers[i].event_id == event_id {
            n += 1;
        }
    }
    n
}

// remove all handlers for event
#[no_mangle]
pub extern "C" fn miku_event_clear(event_id: u32) {
    let mut store = EVENTS.lock();
    for i in 0..store.high_water {
        if store.handlers[i].active && store.handlers[i].event_id == event_id {
            store.handlers[i].active = false;
            store.handlers[i].handler = None;
        }
    }
}

// remove all handlers
#[no_mangle]
pub extern "C" fn miku_event_clear_all() {
    let mut store = EVENTS.lock();
    for i in 0..MAX_HANDLERS {
        store.handlers[i].active = false;
        store.handlers[i].handler = None;
    }
    store.high_water = 0;
}

// register one-shot handler (auto-removed after first call)
#[no_mangle]
pub extern "C" fn miku_event_once(
    event_id: u32,
    handler: EventHandler,
    ctx: *mut u8,
) -> i32 {
    let mut store = EVENTS.lock();
    for i in 0..MAX_HANDLERS {
        if !store.handlers[i].active {
            store.handlers[i] = HandlerEntry {
                event_id,
                handler: Some(handler),
                ctx,
                active: true,
                once: true,
            };
            if i >= store.high_water {
                store.high_water = i + 1;
            }
            return i as i32;
        }
    }
    -1
}

//////////////////////////////////////////////////////////////
//              Event Queue                                 //
// Deferred event dispatch: enqueue events, process later   //
//////////////////////////////////////////////////////////////

const EVENT_QUEUE_SIZE: usize = 128;

struct QueuedEvent {
    event_id: u32,
    data: *mut u8,
}

unsafe impl Send for QueuedEvent {}

const EMPTY_QUEUED: QueuedEvent = QueuedEvent {
    event_id: 0,
    data: core::ptr::null_mut(),
};

struct EventQueue {
    events: [QueuedEvent; EVENT_QUEUE_SIZE],
    head: usize,
    tail: usize,
}

static EVENT_QUEUE: SpinLock<EventQueue> = SpinLock::new(EventQueue {
    events: [EMPTY_QUEUED; EVENT_QUEUE_SIZE],
    head: 0,
    tail: 0,
});

// enqueue event for later dispatch
// Returns 0 on success, -1 if queue full
#[no_mangle]
pub extern "C" fn miku_event_post(event_id: u32, data: *mut u8) -> i32 {
    let mut q = EVENT_QUEUE.lock();
    let next = (q.tail + 1) % EVENT_QUEUE_SIZE;
    if next == q.head {
        return -1; // full
    }
    let idx = q.tail;
    q.events[idx] = QueuedEvent { event_id, data };
    q.tail = next;
    0
}

// process all queued events
// Returns number of events processed
#[no_mangle]
pub extern "C" fn miku_event_flush() -> u32 {
    let mut count = 0u32;
    loop {
        let evt = {
            let mut q = EVENT_QUEUE.lock();
            if q.head == q.tail {
                break;
            }
            let e = QueuedEvent {
                event_id: q.events[q.head].event_id,
                data: q.events[q.head].data,
            };
            q.head = (q.head + 1) % EVENT_QUEUE_SIZE;
            e
        };
        miku_event_emit(evt.event_id, evt.data);
        count += 1;
    }
    count
}

// check if event queue has pending events
#[no_mangle]
pub extern "C" fn miku_event_pending() -> usize {
    let q = EVENT_QUEUE.lock();
    if q.tail >= q.head {
        q.tail - q.head
    } else {
        EVENT_QUEUE_SIZE - q.head + q.tail
    }
}

// clear event queue
#[no_mangle]
pub extern "C" fn miku_event_queue_clear() {
    let mut q = EVENT_QUEUE.lock();
    q.head = 0;
    q.tail = 0;
}
