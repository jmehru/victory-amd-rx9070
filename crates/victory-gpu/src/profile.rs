//! Where the submitting thread's time goes (`VICTORY_PROFILE=<file>`; off by
//! default): a thread of its own stops it about every millisecond, takes its
//! call stack and counts it. From `VICTORY_PROFILE_FROM` seconds after the
//! first submission (150) the stacks of each `VICTORY_PROFILE_SECS` seconds
//! (60) are written to `<file>.<n>`: the functions most seen, in themselves
//! and with what they call, in ms per presented frame, and the whole stacks
//! most seen. The thread that copies back (the relay) is sampled the same way.
//! (After belobug's `profile.rs`; inlined functions are named too.)

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[repr(C, align(16))]
struct Context([u8; 1232]);

const RIP: usize = 0xF8;
const FLAGS: usize = 0x30;
const CONTEXT_FULL: u32 = 0x10_000B;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenThread(access: u32, inherit: i32, id: u32) -> *mut c_void;
    fn SuspendThread(thread: *mut c_void) -> u32;
    fn ResumeThread(thread: *mut c_void) -> u32;
    fn GetThreadContext(thread: *mut c_void, context: *mut Context) -> i32;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentProcess() -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetModuleHandleExW(flags: u32, at: u64, module: *mut u64) -> i32;
    fn GetModuleFileNameW(module: u64, name: *mut u16, len: u32) -> u32;
    fn RtlLookupFunctionEntry(pc: u64, base: *mut u64, history: *mut c_void) -> *mut c_void;
    fn RtlVirtualUnwind(
        kind: u32,
        base: u64,
        pc: u64,
        entry: *mut c_void,
        context: *mut Context,
        data: *mut *mut c_void,
        frame: *mut u64,
        pointers: *mut c_void,
    ) -> *mut c_void;
}

#[link(name = "dbghelp")]
unsafe extern "system" {
    fn SymInitialize(process: *mut c_void, path: *const u8, invade: i32) -> i32;
    fn SymSetOptions(options: u32) -> u32;
    fn SymFromAddr(process: *mut c_void, at: u64, displacement: *mut u64, symbol: *mut u8) -> i32;
    fn SymGetLineFromAddr64(process: *mut c_void, at: u64, displacement: *mut u32, line: *mut u64) -> i32;
    fn SymAddrIncludeInlineTrace(process: *mut c_void, at: u64) -> u32;
    fn SymQueryInlineTrace(
        process: *mut c_void,
        start: u64,
        start_context: u32,
        start_return: u64,
        at: u64,
        context: *mut u32,
        frame: *mut u32,
    ) -> i32;
    fn SymFromInlineContext(process: *mut c_void, at: u64, context: u32, displacement: *mut u64, symbol: *mut u8) -> i32;
    fn SymGetLineFromInlineContext(
        process: *mut c_void,
        at: u64,
        context: u32,
        module: u64,
        displacement: *mut u32,
        line: *mut u64,
    ) -> i32;
}

/// The threads sampled: the one submitting, the one copying back.
const NAMES: [&str; 4] = [
    "submitting thread",
    "copy-back (relay) thread",
    "the threads that help with refreshes, snapshots and compares (summed)",
    "the title's threads that record lists, but the submitting one (summed)",
];
/// The title's threads seen recording a list.
static RECORDERS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// Called by a thread that records into a list (cheap after its first call).
pub(crate) fn recorder_thread() {
    thread_local! {
        static KNOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if KNOWN.get() {
        return;
    }
    KNOWN.set(true);
    if file().is_some() {
        // SAFETY: a plain query.
        RECORDERS.lock().unwrap_or_else(|e| e.into_inner()).push(unsafe { GetCurrentThreadId() });
    }
}
/// The helping threads (`crate::shadow::wide`'s).
static HELPERS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
static THREADS: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
/// Whether the submitting thread is inside a submission.
static INSIDE: AtomicBool = AtomicBool::new(false);
/// Frames presented.
static FRAMES: AtomicU64 = AtomicU64::new(0);

fn file() -> Option<&'static str> {
    static FILE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    FILE.get_or_init(|| {
        let file = std::env::var("VICTORY_PROFILE").ok().filter(|f| !f.is_empty());
        if let Some(file) = file.clone() {
            let _ = std::thread::Builder::new().name("victory-profile".into()).spawn(move || sample(&file));
        }
        file
    })
    .as_deref()
}

/// Until dropped, the thread is in a submission.
pub(crate) struct Inside(bool);

impl Drop for Inside {
    fn drop(&mut self) {
        if self.0 {
            INSIDE.store(false, Ordering::Relaxed);
        }
    }
}

/// Called by the thread that submits, at each submission.
pub(crate) fn here() -> Inside {
    if file().is_none() {
        return Inside(false);
    }
    // SAFETY: a plain query.
    THREADS[0].store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    INSIDE.store(true, Ordering::Relaxed);
    Inside(true)
}

/// Called once by the thread that copies back.
pub(crate) fn relay_thread() {
    if file().is_some() {
        // SAFETY: a plain query.
        THREADS[1].store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    }
}

/// Called once by each thread that helps the submitting one.
pub(crate) fn helper_thread() {
    if file().is_some() {
        // SAFETY: a plain query.
        HELPERS.lock().unwrap_or_else(|e| e.into_inner()).push(unsafe { GetCurrentThreadId() });
    }
}

/// The title's own buffer copies done at submission: how many and their
/// bytes, and of those the ones of 1 MiB and more.
static COPIES: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Called for each of the title's buffer copies done at submission.
pub(crate) fn copied(len: usize) {
    COPIES[0].fetch_add(1, Ordering::Relaxed);
    COPIES[1].fetch_add(len as u64, Ordering::Relaxed);
    if len >= 1 << 20 {
        COPIES[2].fetch_add(1, Ordering::Relaxed);
        COPIES[3].fetch_add(len as u64, Ordering::Relaxed);
    }
}

/// Texture hashes taken: how many and their bytes, and of those the ones
/// of 2 MiB and more.
static HASHED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Called for each hash of a texture's bytes.
pub(crate) fn hashed(len: usize) {
    HASHED[0].fetch_add(1, Ordering::Relaxed);
    HASHED[1].fetch_add(len as u64, Ordering::Relaxed);
    if len >= 2 << 20 {
        HASHED[2].fetch_add(1, Ordering::Relaxed);
        HASHED[3].fetch_add(len as u64, Ordering::Relaxed);
    }
}

/// Frames presented so far.
pub(crate) fn frames() -> u64 {
    FRAMES.load(Ordering::Relaxed)
}

/// Called at each present.
pub(crate) fn presented() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// The registers of the last stack taken (only the sampling thread's).
static mut KEPT: [u64; 7] = [0; 7];

/// Events of the title's that we set (fence events): how often, and the
/// microseconds from the asking to the setting.
pub(crate) static EVENTS_SET: std::sync::Mutex<Option<HashMap<usize, (u64, u64)>>> = std::sync::Mutex::new(None);

/// Called when an event the title waits for a fence with is set, `us`
/// after it asked.
pub(crate) fn event_set(event: usize, us: u64) {
    if file().is_none() {
        return;
    }
    let mut events = EVENTS_SET.lock().unwrap_or_else(|e| e.into_inner());
    let e = events.get_or_insert_with(HashMap::new).entry(event).or_insert((0, 0));
    e.0 += 1;
    e.1 += us;
}

/// The call stack of the thread `handle`, stopped meanwhile, into `stack`
/// (with room for it: nothing is allocated while a thread is stopped, which
/// may hold the heap's lock).
unsafe fn stack_of(handle: *mut c_void, context: &mut Context, stack: &mut Vec<u64>) -> bool {
    // SAFETY: the caller's handle; a context of the right size and
    // alignment; unwinding of a stopped thread's copy of its registers.
    unsafe {
        if SuspendThread(handle) == u32::MAX {
            return false;
        }
        context.0[FLAGS..FLAGS + 4].copy_from_slice(&CONTEXT_FULL.to_ne_bytes());
        if GetThreadContext(handle, context) != 0 {
            // The registers a call keeps, as the innermost frame has them:
            // a wait's handle is in one of them (rbx, rsi, rdi, r12-r15).
            for (k, at) in [0x90usize, 0xA8, 0xB0, 0xD8, 0xE0, 0xE8, 0xF0].into_iter().enumerate() {
                KEPT[k] = u64::from_ne_bytes(context.0[at..at + 8].try_into().expect("8 bytes"));
            }
            while stack.len() + 2 < stack.capacity() {
                let pc = u64::from_ne_bytes(context.0[RIP..RIP + 8].try_into().expect("8 bytes"));
                if pc == 0 {
                    break;
                }
                stack.push(pc);
                let mut base = 0u64;
                let entry = RtlLookupFunctionEntry(pc, &mut base, std::ptr::null_mut());
                if entry.is_null() {
                    // Title code or a leaf: no further.
                    break;
                }
                let (mut data, mut frame) = (std::ptr::null_mut(), 0u64);
                RtlVirtualUnwind(0, base, pc, entry, context, &mut data, &mut frame, std::ptr::null_mut());
            }
        }
        ResumeThread(handle);
    }
    true
}

fn seconds(name: &str, default: u64) -> std::time::Duration {
    std::time::Duration::from_secs(std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default))
}

/// Counts kept elsewhere, since the start: questions to Windows about
/// absent blocks and those answered from kept answers; write-watch askings.
fn counts() -> [u64; 26] {
    std::array::from_fn(|i| match i {
        0 => crate::watch::ABSENT_ASKS.load(Ordering::Relaxed),
        1 => 0,
        2..=5 => crate::watch::KERNEL_ASKS[i - 2].load(Ordering::Relaxed),
        6..=9 => COPIES[i - 6].load(Ordering::Relaxed),
        10..=16 => crate::watch::GUARD_STATS[i - 10].load(Ordering::Relaxed),
        17..=20 => HASHED[i - 17].load(Ordering::Relaxed),
        _ => crate::shadow::POOL_STATS[i - 21].load(Ordering::Relaxed),
    })
}

fn sample(file: &str) {
    const DEPTH: usize = 64;
    let from = seconds("VICTORY_PROFILE_FROM", 150);
    let window = seconds("VICTORY_PROFILE_SECS", 60);
    let mut stacks: [HashMap<Vec<u64>, u32>; 4] = [HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new()];
    let mut helpers: Vec<*mut c_void> = Vec::new();
    let mut handles: [(u32, *mut c_void); 2] = [(0, std::ptr::null_mut()); 2];
    let mut context = Box::new(Context([0; 1232]));
    let mut began: Option<std::time::Instant> = None;
    let mut since: Option<(std::time::Instant, u64)> = None;
    let mut written = 0u32;
    // How many times each was sampled (a sample of the helpers is of all).
    let mut rounds = [0u32; 4];
    let mut recorders: Vec<(u32, *mut c_void)> = Vec::new();
    let mut waits: HashMap<(u64, [u64; 7]), u32> = HashMap::new();
    let mut counted = [0u64; 26];
    loop {
        std::thread::sleep(std::time::Duration::from_millis(1));
        if THREADS[0].load(Ordering::Relaxed) == 0 {
            continue;
        }
        let began = *began.get_or_insert_with(std::time::Instant::now);
        if began.elapsed() < from {
            continue;
        }
        if since.is_none() {
            counted = counts();
        }
        let (start, frames) = *since.get_or_insert_with(|| (std::time::Instant::now(), FRAMES.load(Ordering::Relaxed)));
        for slot in 0..2 {
            let id = THREADS[slot].load(Ordering::Relaxed);
            if id == 0 {
                continue;
            }
            // SAFETY: handles of ours.
            unsafe {
                if id != handles[slot].0 {
                    if !handles[slot].1.is_null() {
                        CloseHandle(handles[slot].1);
                    }
                    // To suspend and resume, get the context and query.
                    handles[slot] = (id, OpenThread(0x2 | 0x8 | 0x40, 0, id));
                }
                if handles[slot].1.is_null() {
                    continue;
                }
                let mut stack = Vec::with_capacity(DEPTH);
                let inside = slot == 0 && INSIDE.load(Ordering::Relaxed);
                if stack_of(handles[slot].1, &mut context, &mut stack) {
                    if slot == 0 && !inside {
                        // What the submitting thread holds while it waits
                        // between submissions.
                        *waits.entry((stack[0], KEPT)).or_insert(0u32) += 1;
                    }
                    stack.push(u64::from(inside));
                    *stacks[slot].entry(stack).or_insert(0) += 1;
                    rounds[slot] += 1;
                }
            }
        }
        // The helpers: every one, into one count. Samples are of threads:
        // three threads busy all the time show three times the frame's ms.
        {
            let ids = HELPERS.lock().unwrap_or_else(|e| e.into_inner());
            while helpers.len() < ids.len() {
                // SAFETY: a plain call.
                helpers.push(unsafe { OpenThread(0x2 | 0x8 | 0x40, 0, ids[helpers.len()]) });
            }
        }
        let mut any = false;
        for &handle in helpers.iter().filter(|h| !h.is_null()) {
            let mut stack = Vec::with_capacity(DEPTH);
            // SAFETY: a handle of ours.
            if unsafe { stack_of(handle, &mut context, &mut stack) } {
                *stacks[2].entry(stack).or_insert(0) += 1;
                any = true;
            }
        }
        rounds[2] += u32::from(any);
        // The title's recording threads likewise (not the submitting one).
        {
            let ids = RECORDERS.lock().unwrap_or_else(|e| e.into_inner());
            while recorders.len() < ids.len() {
                let id = ids[recorders.len()];
                // SAFETY: a plain call.
                recorders.push((id, unsafe { OpenThread(0x2 | 0x8 | 0x40, 0, id) }));
            }
        }
        let submitting = THREADS[0].load(Ordering::Relaxed);
        let mut any = false;
        for &(id, handle) in recorders.iter().filter(|r| !r.1.is_null()) {
            if id == submitting {
                continue;
            }
            let mut stack = Vec::with_capacity(DEPTH);
            // SAFETY: a handle of ours.
            if unsafe { stack_of(handle, &mut context, &mut stack) } {
                *stacks[3].entry(stack).or_insert(0) += 1;
                any = true;
            }
        }
        rounds[3] += u32::from(any);
        if start.elapsed() >= window {
            written += 1;
            let secs = start.elapsed().as_secs_f64();
            let frames = FRAMES.load(Ordering::Relaxed) - frames;
            let mut text = String::new();
            for slot in 0..4 {
                text.push_str(&report(NAMES[slot], &stacks[slot], secs, frames, rounds[slot]));
                stacks[slot].clear();
                rounds[slot] = 0;
            }
            // Counts kept elsewhere, per frame of the window.
            let now = counts();
            let per: [f64; 26] = std::array::from_fn(|i| (now[i] - counted[i]) as f64 / frames.max(1) as f64);
            counted = now;
            text.push_str(&format!(
                "==== per frame: {:.0} questions to Windows about absent blocks;                  write-watch askings made ahead {:.0} answered and {:.0} failed, for one range {:.0} answered and {:.0} failed;                  the title's buffer copies {:.0} of {:.1} MB, of them {:.1} of 1 MiB and more with {:.1} MB
",
                per[0], per[2], per[3], per[4], per[5], per[6], per[7] / 1e6, per[8], per[9] / 1e6
            ));
            text.push_str(&format!(
                "==== per frame: {:.0} checks of ranges by the guards, {:.0} of them with nothing written at once, {:.0} blocks walked by the others, \
                 {:.0} absent blocks met, {:.0} of them in a placeholder and not asked about, {:.2} found committed by a question ({} since the start), \
                 {:.0} runs made read-only\n",
                per[10], per[11], per[12], per[13], per[14], per[15], now[15], per[16]
            ));
            text.push_str(&format!(
                "==== per frame: {:.1} texture hashes of {:.1} MB, of them {:.1} of 2 MiB and more with {:.1} MB\n",
                per[17], per[18] / 1e6, per[19], per[20] / 1e6
            ));
            text.push_str(&format!(
                "==== per frame: {:.1} snapshot buffers asked for, {:.1} of them made new with {:.1} MB of room; {:.1} let go by the pool with {:.1} MB\n",
                per[21], per[22], per[23] / 1e6, per[24], per[25] / 1e6
            ));
            // Between submissions: where the submitting thread stood and the
            // registers it kept there, most seen first; and the events of
            // the title's that we set.
            let mut all: Vec<_> = waits.drain().collect();
            all.sort_unstable_by(|a, b| b.1.cmp(&a.1));
            text.push_str("==== between submissions: samples, where, rbx rsi rdi r12 r13 r14 r15\n");
            for ((at, kept), n) in all.into_iter().take(12) {
                text.push_str(&format!("{n:7} {at:#x} {kept:x?}\n"));
            }
            let events = EVENTS_SET.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
            let mut events: Vec<_> = events.into_iter().collect();
            events.sort_unstable_by(|a, b| b.1.1.cmp(&a.1.1));
            text.push_str(&format!("==== events set for fences in the window, {} frames: handle, times, ms from asked to set in all\n", frames));
            for (event, (n, us)) in events.into_iter().take(12) {
                text.push_str(&format!("{event:#x} {n} {:.1}\n", us as f64 / 1e3));
            }
            let _ = std::fs::write(format!("{file}.{written}"), text);
            since = None;
        }
    }
}

/// The names at a code address, innermost first: the functions inlined
/// there, then the function itself; and the source line of the innermost.
/// `leaf` is false for a return address, looked up a byte before.
struct Namer {
    names: HashMap<(u64, bool), (Vec<String>, u32)>,
}

impl Namer {
    fn new() -> Self {
        // SAFETY: plain calls.
        unsafe {
            // Undecorated names, loaded when asked for, with lines.
            SymSetOptions(0x2 | 0x4 | 0x10);
            let mut path = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()))
                .unwrap_or_default()
                .into_bytes();
            path.push(0);
            SymInitialize(GetCurrentProcess(), path.as_ptr(), 1);
        }
        Namer { names: HashMap::new() }
    }

    fn of(&mut self, at: u64, leaf: bool) -> &(Vec<String>, u32) {
        self.names.entry((at, leaf)).or_insert_with(|| Self::look(at, leaf))
    }

    fn look(at: u64, leaf: bool) -> (Vec<String>, u32) {
        if at < 2 {
            return (vec![["[between submissions]", "[in a submission]"][at as usize].to_string()], 0);
        }
        let at = if leaf { at } else { at - 1 };
        // SAFETY: plain calls; the symbol record has room for the name.
        unsafe {
            let process = GetCurrentProcess();
            // The module the address is in, by address and without a reference.
            let mut module = 0u64;
            let mut file = [0u16; 260];
            let module_name = if GetModuleHandleExW(0x4 | 0x2, at, &mut module) != 0 {
                let n = GetModuleFileNameW(module, file.as_mut_ptr(), 260) as usize;
                let path = String::from_utf16_lossy(&file[..n]);
                path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
            } else {
                String::new()
            };
            if module == 0 {
                return (vec!["(title code)".to_string()], 0);
            }
            let ours = module_name.eq_ignore_ascii_case("victory.exe");
            let text = |symbol: &[u8]| {
                let len = u32::from_ne_bytes(symbol[76..80].try_into().expect("4 bytes")) as usize;
                let name = String::from_utf8_lossy(&symbol[84..84 + len.min(500)]).into_owned();
                if ours { name } else { format!("{module_name}!{name}") }
            };
            let fresh = || {
                let mut symbol = [0u8; 88 + 512];
                symbol[0..4].copy_from_slice(&88u32.to_ne_bytes());
                symbol[80..84].copy_from_slice(&500u32.to_ne_bytes());
                symbol
            };
            let mut names = Vec::new();
            let mut line_no = 0u32;
            let mut line = [0u64; 5];
            line[0] = 40;
            let mut off = 0u32;
            if ours {
                let inlined = SymAddrIncludeInlineTrace(process, at);
                let (mut context, mut frame) = (0u32, 0u32);
                if inlined > 0 && SymQueryInlineTrace(process, at, 0, at, at, &mut context, &mut frame) != 0 {
                    for i in 0..inlined {
                        let mut symbol = fresh();
                        let mut displacement = 0u64;
                        if SymFromInlineContext(process, at, context + i, &mut displacement, symbol.as_mut_ptr()) != 0 {
                            names.push(text(&symbol));
                        }
                    }
                    if SymGetLineFromInlineContext(process, at, context, 0, &mut off, line.as_mut_ptr()) != 0 {
                        line_no = line[2] as u32;
                    }
                }
                if line_no == 0 && SymGetLineFromAddr64(process, at, &mut off, line.as_mut_ptr()) != 0 {
                    line_no = line[2] as u32;
                }
            }
            let mut symbol = fresh();
            let mut displacement = 0u64;
            if SymFromAddr(process, at, &mut displacement, symbol.as_mut_ptr()) != 0 {
                let name = text(&symbol);
                if names.last() != Some(&name) {
                    names.push(name);
                }
            } else {
                names.push(format!("{module_name}+{:#x}", (at - module) & !0xfff));
            }
            (names, line_no)
        }
    }
}

/// The report of one thread's stacks, `secs` seconds and `frames` frames long.
fn report(thread: &str, stacks: &HashMap<Vec<u64>, u32>, secs: f64, frames: u64, rounds: u32) -> String {
    static NAMER: std::sync::Mutex<Option<Namer>> = std::sync::Mutex::new(None);
    let mut namer = NAMER.lock().unwrap_or_else(|e| e.into_inner());
    let namer = namer.get_or_insert_with(Namer::new);
    let mut own: HashMap<String, u32> = HashMap::new();
    let mut own_line: HashMap<String, u32> = HashMap::new();
    let mut with: HashMap<String, u32> = HashMap::new();
    let mut edges: HashMap<String, u32> = HashMap::new();
    let mut total = 0u32;
    // Each stack as names, innermost first.
    let mut named: Vec<(Vec<String>, u32)> = Vec::new();
    // The same with the line of each code address (for the stacks).
    let mut lined: Vec<(Vec<String>, u32)> = Vec::new();
    for (stack, &n) in stacks {
        total += n;
        let mut frames_named = Vec::new();
        let mut frames_lined = Vec::new();
        for (depth, &at) in stack.iter().enumerate() {
            let (names, line) = namer.of(at, depth == 0).clone();
            if depth == 0 {
                *own.entry(names[0].clone()).or_insert(0) += n;
                *own_line.entry(format!("{}:{line} (in {})", names[0], names.last().expect("a name"))).or_insert(0) += n;
            }
            frames_lined.extend(names.iter().enumerate().map(|(i, name)| {
                if i == 0 && line != 0 { format!("{name}:{line}") } else { name.clone() }
            }));
            frames_named.extend(names);
        }
        let mut seen = std::collections::HashSet::new();
        let mut seen_edges = std::collections::HashSet::new();
        for (i, name) in frames_named.iter().enumerate() {
            if seen.insert(name.clone()) {
                *with.entry(name.clone()).or_insert(0) += n;
            }
            if let Some(caller) = frames_named.get(i + 1) {
                let edge = format!("{name}  <-  {caller}");
                if seen_edges.insert(edge.clone()) {
                    *edges.entry(edge).or_insert(0) += n;
                }
            }
        }
        named.push((frames_named, n));
        lined.push((frames_lined, n));
    }
    let frames = frames.max(1) as f64;
    // One sample stands for this many ms of a frame.
    let per = secs * 1000.0 / frames / f64::from(rounds.max(1));
    let mut text = format!(
        "==== {thread}: {total} samples in {secs:.1} s, {frames:.0} frames ({:.1} ms/frame); one sample = {per:.4} ms/frame\n",
        secs * 1000.0 / frames
    );
    for (title, counts, most) in [
        ("in itself (innermost function, inlined ones named)", own, 70),
        ("in itself by line", own_line, 120),
        ("with what it calls", with, 160),
        ("with what it calls, by caller (callee <- caller)", edges, 220),
    ] {
        let mut all: Vec<_> = counts.into_iter().collect();
        all.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        text.push_str(&format!("-- {title}: ms/frame, share, name\n"));
        for (name, n) in all.into_iter().take(most) {
            let share = f64::from(n) * 100.0 / f64::from(total.max(1));
            text.push_str(&format!("{:7.2} {share:5.1}% {name}\n", f64::from(n) * per));
        }
    }
    drop(named);
    let mut named = lined;
    named.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    text.push_str("-- stacks: ms/frame, innermost first\n");
    for (frames_named, n) in named.iter().take(120) {
        let shown: Vec<&str> = frames_named.iter().take(28).map(String::as_str).collect();
        text.push_str(&format!("{:7.2} {}\n", f64::from(*n) * per, shown.join(" < ")));
    }
    text
}
