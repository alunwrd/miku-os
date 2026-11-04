// Host check of `paint.cu` against the processor's painter, through CUDA
//
// nvcc compiles `paint.cu` with the flags on its first line (`lib/buildgen`,
// `file_flags`).
//
// `AOSH_GPU_CHECK=1` draws every rectangle of every paint on both and reports
// differences (`Scene::check_gpu`). `libcuda` opens on first use, so nothing
// links against it

use std::ffi::{c_void, CString};
use std::path::{Path, PathBuf};

use super::record::{Memory, BLOCK_THREADS};

/// `AOSH_GPU_CHECK=1`
pub fn wanted() -> bool {
    std::env::var("AOSH_GPU_CHECK").is_ok_and(|v| v == "1")
}

type CuResult = i32;
type Fp = *mut c_void;

extern "C" {
    fn dlopen(name: *const i8, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const i8) -> *mut c_void;
}

/// The driver's functions this needs, by name
struct Api {
    mem_alloc: unsafe extern "C" fn(*mut u64, usize) -> CuResult,
    mem_free: unsafe extern "C" fn(u64) -> CuResult,
    htod: unsafe extern "C" fn(u64, *const c_void, usize) -> CuResult,
    dtoh: unsafe extern "C" fn(*mut c_void, u64, usize) -> CuResult,
    launch: unsafe extern "C" fn(Fp, u32, u32, u32, u32, u32, u32, u32, Fp, *mut *mut c_void, *mut *mut c_void) -> CuResult,
    sync: unsafe extern "C" fn() -> CuResult,
}

/// The card, the program and three buffers, made once a test thread
pub struct Cuda {
    api: Api,
    func: Fp,
    /// `(device address, bytes)`
    cmd: (u64, usize),
    tex: (u64, usize),
    dst: (u64, usize),
    /// What the last `go` spent sending the commands, and running
    pub last_upload: std::time::Duration,
    pub last_run: std::time::Duration,
}

/// Host mirror of texture memory, sent to the card before each run. Shared by
/// the recording and the run
pub struct Host {
    pub bytes: Vec<u8>,
    pub dirty: Vec<(usize, usize)>,
}

pub struct HostMemory(pub std::rc::Rc<std::cell::RefCell<Host>>);

/// Room for textures, on the host and the card alike
pub const TEX_BYTES: usize = 256 << 20;

impl HostMemory {
    pub fn new() -> HostMemory {
        HostMemory(std::rc::Rc::new(std::cell::RefCell::new(Host { bytes: vec![0; TEX_BYTES], dirty: Vec::new() })))
    }
}

impl Memory for HostMemory {
    fn size(&self) -> usize {
        self.0.borrow().bytes.len()
    }
    fn write(&mut self, at: usize, bytes: &[u8]) {
        let mut h = self.0.borrow_mut();
        h.bytes[at..at + bytes.len()].copy_from_slice(bytes);
        h.dirty.push((at, bytes.len()));
    }
}

fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../kernel/drivers/gpu/nvidia/shaders/paint.cu")
}

/// `paint.cu` compiled as the kernel's build compiles it
fn cubin() -> Result<Vec<u8>, String> {
    let src = source();
    let first = std::fs::read_to_string(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    let flags: Vec<String> = first.lines().next().and_then(|l| l.strip_prefix("// nvcc:"))
        .map(|f| f.split_whitespace().map(str::to_string).collect()).unwrap_or_default();
    let out = std::env::temp_dir().join(format!("aosh-paint-{}.cubin", std::process::id()));
    let nvcc = ["/opt/cuda/bin/nvcc", "/usr/local/cuda/bin/nvcc", "nvcc"].into_iter()
        .find(|p| !p.starts_with('/') || Path::new(p).exists()).unwrap_or("nvcc");
    let made = std::process::Command::new(nvcc).args(["-arch", "sm_120", "-cubin"]).args(&flags)
        .arg("-o").arg(&out).arg(&src).output().map_err(|e| format!("{nvcc}: {e}"))?;
    if !made.status.success() {
        return Err(format!("nvcc: {}", String::from_utf8_lossy(&made.stderr)));
    }
    let bytes = std::fs::read(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let _ = std::fs::remove_file(&out);
    Ok(bytes)
}

unsafe fn sym<T>(lib: *mut c_void, name: &str) -> Result<T, String> {
    let n = CString::new(name).unwrap();
    let p = dlsym(lib, n.as_ptr());
    if p.is_null() {
        return Err(format!("libcuda has no {name}"));
    }
    Ok(std::mem::transmute_copy(&p))
}

fn ok(r: CuResult, what: &str) -> Result<(), String> {
    if r == 0 { Ok(()) } else { Err(format!("{what}: CUDA error {r}")) }
}

impl Cuda {
    pub fn open() -> Result<Cuda, String> {
        unsafe {
            let name = CString::new("libcuda.so.1").unwrap();
            let lib = dlopen(name.as_ptr(), 2);
            if lib.is_null() {
                return Err("no libcuda.so.1".into());
            }
            let init: unsafe extern "C" fn(u32) -> CuResult = sym(lib, "cuInit")?;
            let device_get: unsafe extern "C" fn(*mut i32, i32) -> CuResult = sym(lib, "cuDeviceGet")?;
            let retain: unsafe extern "C" fn(*mut Fp, i32) -> CuResult = sym(lib, "cuDevicePrimaryCtxRetain")?;
            let set: unsafe extern "C" fn(Fp) -> CuResult = sym(lib, "cuCtxSetCurrent")?;
            let load: unsafe extern "C" fn(*mut Fp, *const c_void) -> CuResult = sym(lib, "cuModuleLoadData")?;
            let get: unsafe extern "C" fn(*mut Fp, Fp, *const i8) -> CuResult = sym(lib, "cuModuleGetFunction")?;
            let api = Api {
                mem_alloc: sym(lib, "cuMemAlloc_v2")?,
                mem_free: sym(lib, "cuMemFree_v2")?,
                htod: sym(lib, "cuMemcpyHtoD_v2")?,
                dtoh: sym(lib, "cuMemcpyDtoH_v2")?,
                launch: sym(lib, "cuLaunchKernel")?,
                sync: sym(lib, "cuCtxSynchronize")?,
            };
            ok(init(0), "cuInit")?;
            let mut dev = 0;
            ok(device_get(&mut dev, 0), "cuDeviceGet")?;
            let mut ctx: Fp = std::ptr::null_mut();
            ok(retain(&mut ctx, dev), "cuDevicePrimaryCtxRetain")?;
            ok(set(ctx), "cuCtxSetCurrent")?;
            let code = cubin()?;
            let mut module: Fp = std::ptr::null_mut();
            ok(load(&mut module, code.as_ptr() as *const c_void), "cuModuleLoadData")?;
            let mut func: Fp = std::ptr::null_mut();
            let fname = CString::new("paint").unwrap();
            ok(get(&mut func, module, fname.as_ptr()), "cuModuleGetFunction")?;
            let mut c = Cuda { api, func, cmd: (0, 0), tex: (0, 0), dst: (0, 0), last_upload: Default::default(), last_run: Default::default() };
            c.tex = c.alloc(TEX_BYTES)?;
            Ok(c)
        }
    }

    fn alloc(&self, bytes: usize) -> Result<(u64, usize), String> {
        let mut p = 0u64;
        ok(unsafe { (self.api.mem_alloc)(&mut p, bytes.max(4)) }, "cuMemAlloc")?;
        Ok((p, bytes.max(4)))
    }

    fn room(&self, have: (u64, usize), bytes: usize) -> Result<(u64, usize), String> {
        if have.1 >= bytes && have.0 != 0 {
            return Ok(have);
        }
        if have.0 != 0 {
            unsafe { (self.api.mem_free)(have.0) };
        }
        self.alloc(bytes.next_power_of_two())
    }

    /// Sends textures written since the last run
    pub fn textures(&mut self, mem: &HostMemory) -> Result<(), String> {
        let mut h = mem.0.borrow_mut();
        for (at, len) in std::mem::take(&mut h.dirty) {
            ok(unsafe { (self.api.htod)(self.tex.0 + at as u64, h.bytes[at..].as_ptr() as *const c_void, len) }, "textures")?;
        }
        Ok(())
    }

    /// Runs `words` over `tiles` blocks into a `dst_bytes` target starting as
    /// `fill`; returns the target
    pub fn run(&mut self, words: &[u32], tiles: u32, dst_bytes: usize, fill: u8) -> Result<Vec<u8>, String> {
        self.go(words, tiles, dst_bytes, Some(fill))?;
        let mut out = vec![0u8; dst_bytes];
        ok(unsafe { (self.api.dtoh)(out.as_mut_ptr() as *mut c_void, self.dst.0, dst_bytes) }, "reading back")?;
        Ok(out)
    }

    /// Runs `words` and waits, reading nothing back (`shot::bench`,
    /// `AOSH_BENCH_GPU=1`)
    pub fn go(&mut self, words: &[u32], tiles: u32, dst_bytes: usize, fill: Option<u8>) -> Result<(), String> {
        self.cmd = self.room(self.cmd, words.len() * 4)?;
        self.dst = self.room(self.dst, dst_bytes)?;
        let t = std::time::Instant::now();
        ok(unsafe { (self.api.htod)(self.cmd.0, words.as_ptr() as *const c_void, words.len() * 4) }, "commands")?;
        self.last_upload = t.elapsed();
        if let Some(fill) = fill {
            let start = vec![fill; dst_bytes];
            ok(unsafe { (self.api.htod)(self.dst.0, start.as_ptr() as *const c_void, dst_bytes) }, "target")?;
        }
        if tiles > 0 {
            // One target pool, standing in for all four
            let (d, n) = (self.dst.0, dst_bytes as u64);
            let mut args: [u64; 12] = [self.cmd.0, (words.len() * 4) as u64, self.tex.0, TEX_BYTES as u64, d, n, d, n, d, n, d, n];
            let mut ptrs: Vec<*mut c_void> = args.iter_mut().map(|a| a as *mut u64 as *mut c_void).collect();
            ok(unsafe {
                (self.api.launch)(self.func, tiles, 1, 1, BLOCK_THREADS, 1, 1, 0, std::ptr::null_mut(), ptrs.as_mut_ptr(),
                                  std::ptr::null_mut())
            }, "cuLaunchKernel")?;
            let t = std::time::Instant::now();
            ok(unsafe { (self.api.sync)() }, "the program")?;
            self.last_run = t.elapsed();
        }
        Ok(())
    }
}
