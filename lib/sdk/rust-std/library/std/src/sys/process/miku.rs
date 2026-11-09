//! `std::process::Command` on MikuOS.
//!
//! `fork` then `execve`: `fork`(43) makes a copy-on-write child, `execve`(67)
//! replaces its image, and `wait4`(44) reports what became of it in a POSIX
//! status word.
//!
//! The child is single-threaded, since `fork` gives it one task, the caller.
//! Everything between the two runs in a copy of this program's address space,
//! so it allocates nothing: the argument and environment vectors are built
//! before the fork.

use super::CommandEnvs;
use super::env::{CommandEnv, CommandResolvedEnvs};
use crate::ffi::OsStr;
pub use crate::ffi::OsString as EnvKey;
use crate::num::NonZeroI32;
use crate::os::fd::{AsRawFd, FromRawFd, RawFd};
use crate::os::miku::ffi::OsStrExt;
use crate::path::Path;
use crate::process::StdioPipes;
use crate::sys::fd::FileDesc;
use crate::sys::fs::File;
use crate::sys::pal::{abi, cvt};
use crate::sys::{AsInner, FromInner, IntoInner};
use crate::{fmt, io};

/// A NUL-terminated copy of an `OsStr`, for the kernel's C strings
fn cstr(s: &OsStr) -> io::Result<Vec<u8>> {
    let b = s.as_bytes();
    if b.iter().any(|&c| c == 0) {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "argument contains a NUL byte",
        ));
    }
    let mut v = Vec::with_capacity(b.len() + 1);
    v.extend_from_slice(b);
    v.push(0);
    Ok(v)
}

pub enum Stdio {
    Inherit,
    Null,
    MakePipe,
    Fd(FileDesc),
}

impl Stdio {
    fn try_clone(&self) -> io::Result<Self> {
        Ok(match self {
            Stdio::Fd(fd) => Stdio::Fd(FileDesc::from_inner(fd.as_inner().try_clone()?)),
            Stdio::Inherit => Stdio::Inherit,
            Stdio::Null => Stdio::Null,
            Stdio::MakePipe => Stdio::MakePipe,
        })
    }

    /// Resolve into the descriptor the child should see on `target`, plus the
    /// end this process keeps.
    ///
    /// `Inherit` is `None` on both counts: `fork` copies the descriptor table.
    fn resolve(self, target: RawFd) -> io::Result<(Option<FileDesc>, Option<FileDesc>)> {
        match self {
            Stdio::Inherit => Ok((None, None)),
            Stdio::Fd(fd) => Ok((Some(fd), None)),
            Stdio::MakePipe => {
                let (r, w) = crate::sys::pipe::pipe()?;
                // stdin: the child reads, we write. Everything else: the other
                // way round
                if target == 0 { Ok((Some(r), Some(w))) } else { Ok((Some(w), Some(r))) }
            }
            Stdio::Null => {
                let path = b"/dev/null\0";
                let fd = cvt(abi::miku_open(path.as_ptr(), path.len() - 1,
                                            abi::OPEN_READ | abi::OPEN_WRITE | abi::OPEN_CLOEXEC, 0))?;
                // SAFETY: freshly opened and owned by nobody else
                Ok((Some(unsafe { FileDesc::from_raw_fd(fd as RawFd) }), None))
            }
        }
    }
}

impl From<FileDesc> for Stdio {
    fn from(fd: FileDesc) -> Stdio {
        Stdio::Fd(fd)
    }
}

impl From<File> for Stdio {
    fn from(file: File) -> Stdio {
        Stdio::Fd(file.into_inner())
    }
}

impl From<io::Stdout> for Stdio {
    fn from(_: io::Stdout) -> Stdio {
        Stdio::Inherit
    }
}

impl From<io::Stderr> for Stdio {
    fn from(_: io::Stderr) -> Stdio {
        Stdio::Inherit
    }
}

pub struct Command {
    program: Vec<u8>,
    args: Vec<Vec<u8>>,
    /// `args[0]` is the program by convention, kept beside them
    arg_osstr: Vec<crate::ffi::OsString>,
    cwd: Option<Vec<u8>>,
    cwd_osstr: Option<crate::ffi::OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
    env: CommandEnv,
    program_osstr: crate::ffi::OsString,
    // `CommandExt`: what the child does to itself between fork and exec
    uid: Option<u32>,
    gid: Option<u32>,
    pgroup: Option<i32>,
    closures: Vec<Box<dyn FnMut() -> io::Result<()> + Send + Sync>>,
}

/// Everything the child needs, built before the fork; see `spawn`
struct Prepared {
    /// Where to look for the program: the one path given, or each `PATH`
    /// directory joined to a bare name, in order
    candidates: Vec<Vec<u8>>,
    argv: Vec<*const u8>,
    envp: Vec<*const u8>,
    _env_bufs: Vec<Vec<u8>>,
}

const F_SETFD: u64 = crate::sys::pal::abi::defs::fcntl::F_SETFD as u64;
const FD_CLOEXEC: i64 = crate::sys::pal::abi::defs::fcntl::FD_CLOEXEC as i64;
use crate::sys::pal::abi::defs::errno::ENOENT;
use crate::sys::pal::abi::defs::errno::ENOTDIR;
use crate::sys::pal::abi::defs::errno::EINVAL;

impl Command {
    pub fn new(program: &OsStr) -> Command {
        let p = cstr(program).unwrap_or_else(|_| vec![0]);
        Command {
            program: p.clone(),
            args: vec![p],
            arg_osstr: Vec::new(),
            cwd: None,
            cwd_osstr: None,
            stdin: None,
            stdout: None,
            stderr: None,
            env: CommandEnv::default(),
            program_osstr: program.to_os_string(),
            uid: None,
            gid: None,
            pgroup: None,
            closures: Vec::new(),
        }
    }

    pub fn set_arg_0(&mut self, arg: &OsStr) {
        if let Ok(c) = cstr(arg) {
            self.args[0] = c;
        }
    }

    pub fn uid(&mut self, id: u32) {
        self.uid = Some(id);
    }

    pub fn gid(&mut self, id: u32) {
        self.gid = Some(id);
    }

    pub fn pgroup(&mut self, pgroup: i32) {
        self.pgroup = Some(pgroup);
    }

    pub fn pre_exec(&mut self, f: Box<dyn FnMut() -> io::Result<()> + Send + Sync>) {
        self.closures.push(f);
    }

    /// The argument and environment vectors, and the paths to try.
    ///
    /// A name without a `/` is looked up as `execvp` does, in the `PATH` the
    /// child will have: the command's own if it set one, this process's
    /// otherwise, `/bin` if neither. `execve`(67) takes a path and nothing
    /// else.
    fn prepare(&self) -> Prepared {
        let envs = self.env.capture();
        let mut env_bufs: Vec<Vec<u8>> = Vec::with_capacity(envs.len());
        let mut path_var: Option<Vec<u8>> = None;
        for (k, v) in envs.iter() {
            if k.as_os_str().as_bytes() == b"PATH" {
                path_var = Some(v.as_bytes().to_vec());
            }
            let mut e = Vec::new();
            e.extend_from_slice(k.as_os_str().as_bytes());
            e.push(b'=');
            e.extend_from_slice(v.as_bytes());
            e.push(0);
            env_bufs.push(e);
        }

        // Without its terminator
        let name = &self.program[..self.program.len() - 1];
        let candidates = if name.contains(&b'/') || name.is_empty() {
            vec![self.program.clone()]
        } else {
            let path = path_var.unwrap_or_else(|| b"/bin".to_vec());
            path.split(|&c| c == b':')
                .map(|dir| {
                    // An empty element is the current directory, as POSIX has
                    // it
                    let dir: &[u8] = if dir.is_empty() { b"." } else { dir };
                    let mut c = Vec::with_capacity(dir.len() + name.len() + 2);
                    c.extend_from_slice(dir);
                    c.push(b'/');
                    c.extend_from_slice(name);
                    c.push(0);
                    c
                })
                .collect()
        };

        let argv: Vec<*const u8> = self.args.iter().map(|a| a.as_ptr()).collect();
        let envp: Vec<*const u8> = env_bufs.iter().map(|e| e.as_ptr()).collect();
        Prepared { candidates, argv, envp, _env_bufs: env_bufs }
    }

    /// Become the program: what the child does after the fork, and what
    /// `CommandExt::exec` does in place. Returns only on failure, with the
    /// errno to report.
    ///
    /// Allocates nothing of its own; a `pre_exec` closure may.
    fn become_program(
        &mut self,
        prep: &Prepared,
        stdin: Option<&FileDesc>,
        stdout: Option<&FileDesc>,
        stderr: Option<&FileDesc>,
    ) -> i32 {
        for (fd, target) in [(stdin, 0), (stdout, 1), (stderr, 2)] {
            if let Some(fd) = fd {
                let r = abi::miku_dup2(fd.as_raw_fd() as i64, target);
                if r < 0 { return -r as i32; }
            }
        }
        if let Some(dir) = &self.cwd {
            let r = abi::miku_chdir(dir.as_ptr());
            if r < 0 { return -r as i32; }
        }
        if let Some(pg) = self.pgroup {
            let r = abi::miku_setpgid(0, pg as u64);
            if r < 0 { return -r as i32; }
        }
        // The group before the user: setting the group needs the uid not yet
        // dropped
        if let Some(g) = self.gid {
            let r = abi::miku_setgid(g as u64);
            if r < 0 { return -r as i32; }
        }
        if let Some(u) = self.uid {
            let r = abi::miku_setuid(u as u64);
            if r < 0 { return -r as i32; }
        }
        // Before the caller's own closures, as upstream orders it, so a
        // `pre_exec` can still ignore SIGPIPE in the child
        crate::sys::pal::child_sigpipe_default();
        for f in self.closures.iter_mut() {
            if let Err(e) = f() {
                return e.raw_os_error().unwrap_or(EINVAL);
            }
        }

        // `execvp`'s rule: a missing file or directory means "try the next
        // one", and any other failure is the answer if nothing succeeds
        let mut err = ENOENT;
        for path in &prep.candidates {
            let r = abi::miku_execve(
                path.as_ptr(),
                prep.argv.as_ptr(),
                prep.argv.len(),
                prep.envp.as_ptr(),
                prep.envp.len(),
            );
            let e = if r < 0 { -r as i32 } else { ENOENT };
            if e != ENOENT && e != ENOTDIR {
                err = e;
            }
        }
        err
    }

    /// `CommandExt::exec`: this process becomes the program, or learns why not
    pub fn exec(&mut self, default: Stdio) -> io::Error {
        let stdin = match self.stdin.as_ref() {
            Some(s) => s.try_clone(),
            None => default.try_clone(),
        };
        let stdout = match self.stdout.as_ref() {
            Some(s) => s.try_clone(),
            None => default.try_clone(),
        };
        let stderr = match self.stderr.as_ref() {
            Some(s) => s.try_clone(),
            None => default.try_clone(),
        };
        let (stdin, stdout, stderr) = match (stdin, stdout, stderr) {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
        };
        // A pipe to nobody has no reader here and no parent side to take the
        // other end, so it is refused
        if matches!(stdin, Stdio::MakePipe)
            || matches!(stdout, Stdio::MakePipe)
            || matches!(stderr, Stdio::MakePipe)
        {
            return io::const_error!(
                io::ErrorKind::InvalidInput,
                "Stdio::piped() has no meaning for exec: nothing would hold the other end",
            );
        }
        let resolved = (stdin.resolve(0), stdout.resolve(1), stderr.resolve(2));
        let (child_in, child_out, child_err) = match resolved {
            (Ok((a, _)), Ok((b, _)), Ok((c, _))) => (a, b, c),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
        };
        let prep = self.prepare();
        let errno = self.become_program(
            &prep, child_in.as_ref(), child_out.as_ref(), child_err.as_ref());
        io::Error::from_raw_os_error(errno)
    }

    pub fn arg(&mut self, arg: &OsStr) {
        if let Ok(c) = cstr(arg) {
            self.args.push(c);
            self.arg_osstr.push(arg.to_os_string());
        }
    }

    pub fn env_mut(&mut self) -> &mut CommandEnv {
        &mut self.env
    }

    pub fn cwd(&mut self, dir: &OsStr) {
        self.cwd = cstr(dir).ok();
        self.cwd_osstr = Some(dir.to_os_string());
    }

    pub fn stdin(&mut self, stdin: Stdio) {
        self.stdin = Some(stdin);
    }
    pub fn stdout(&mut self, stdout: Stdio) {
        self.stdout = Some(stdout);
    }
    pub fn stderr(&mut self, stderr: Stdio) {
        self.stderr = Some(stderr);
    }

    pub fn get_program(&self) -> &OsStr {
        &self.program_osstr
    }

    pub fn get_args(&self) -> CommandArgs<'_> {
        CommandArgs { iter: self.arg_osstr.iter() }
    }

    pub fn get_envs(&self) -> CommandEnvs<'_> {
        self.env.iter()
    }

    pub fn get_env_clear(&self) -> bool {
        self.env.does_clear()
    }

    pub fn get_resolved_envs(&self) -> CommandResolvedEnvs {
        CommandResolvedEnvs::new(self.env.capture())
    }

    pub fn get_current_dir(&self) -> Option<&Path> {
        self.cwd_osstr.as_ref().map(Path::new)
    }

    pub fn spawn(
        &mut self,
        default: Stdio,
        needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        let stdin = match self.stdin.as_ref() {
            Some(s) => s.try_clone()?,
            None if needs_stdin => default.try_clone()?,
            None => Stdio::Null,
        };
        let stdout = match self.stdout.as_ref() {
            Some(s) => s.try_clone()?,
            None => default.try_clone()?,
        };
        let stderr = match self.stderr.as_ref() {
            Some(s) => s.try_clone()?,
            None => default.try_clone()?,
        };

        let (child_in, ours_in) = stdin.resolve(0)?;
        let (child_out, ours_out) = stdout.resolve(1)?;
        let (child_err, ours_err) = stderr.resolve(2)?;

        // Everything the child needs, built before the fork: after it this code
        // runs in a copy of the address space and does not allocate
        let prep = self.prepare();

        // How the child says its exec failed. Both ends are close-on-exec: a
        // successful `execve` closes the child's write end and the parent reads
        // end-of-file; a failed one leaves it open for the child to write the
        // errno into before it exits
        let (err_r, err_w) = crate::sys::pipe::pipe()?;
        cvt(abi::miku_fcntl(err_r.as_raw_fd() as i64, F_SETFD, FD_CLOEXEC))?;
        cvt(abi::miku_fcntl(err_w.as_raw_fd() as i64, F_SETFD, FD_CLOEXEC))?;

        let pid = cvt(abi::miku_fork())?;
        if pid == 0 {
            // The child
            //
            // Nothing here returns: every failure is an exit, or a second copy
            // of the parent would run the parent's code
            drop(err_r);
            let errno = self.become_program(
                &prep, child_in.as_ref(), child_out.as_ref(), child_err.as_ref());
            let bytes = errno.to_le_bytes();
            abi::miku_write(err_w.as_raw_fd() as u64, bytes.as_ptr(), 4);
            // 127 is what a shell reports for "command not found"
            abi::miku_exit(127);
        }

        drop(err_w);
        let mut got = [0u8; 4];
        let mut n = 0;
        while n < 4 {
            match err_r.read(&mut got[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        if n == 4 {
            // The child failed and is exiting; reap it, so a failed spawn
            // leaves no zombie
            let mut status: i64 = 0;
            abi::miku_wait4(pid as u64, &mut status, 0);
            return Err(io::Error::from_raw_os_error(i32::from_le_bytes(got)));
        }

        // The parent
        //
        // The child's ends are closed here, not left to `Drop`: a pipe whose
        // write end is open in this process never reports EOF to the reader
        drop(child_in);
        drop(child_out);
        drop(child_err);

        Ok((
            Process { pid: pid as u64, status: None },
            StdioPipes {
                stdin: ours_in,
                stdout: ours_out,
                stderr: ours_err,
            },
        ))
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.program_osstr)?;
        for a in &self.arg_osstr {
            write!(f, " {a:?}")?;
        }
        Ok(())
    }
}

// exit status

/// The POSIX status word `wait4`(44) writes.
///
/// The kernel keeps the signal beside the code, so `signal()` below reads a
/// recorded fact. `128 + sig` cannot be told from `exit(148)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ExitStatus(i32);

impl ExitStatus {
    pub fn exit_ok(&self) -> Result<(), ExitStatusError> {
        if self.code() == Some(0) { Ok(()) } else { Err(ExitStatusError(*self)) }
    }

    pub fn code(&self) -> Option<i32> {
        // WIFEXITED / WEXITSTATUS
        if self.0 & 0x7f == 0 { Some((self.0 >> 8) & 0xff) } else { None }
    }

    pub fn signal(&self) -> Option<i32> {
        let s = self.0 & 0x7f;
        if s != 0 && s != 0x7f { Some(s) } else { None }
    }

    pub fn core_dumped(&self) -> bool {
        // MikuOS writes a core file for a crash (`/tmp/core.<name>.<pid>`), but
        // the status word has no bit for it
        false
    }

    pub fn stopped_signal(&self) -> Option<i32> {
        if self.0 & 0xff == 0x7f { Some((self.0 >> 8) & 0xff) } else { None }
    }

    pub fn continued(&self) -> bool {
        false
    }

    pub fn into_raw(&self) -> i32 {
        self.0
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(c) = self.code() {
            write!(f, "exit status: {c}")
        } else if let Some(s) = self.signal() {
            write!(f, "killed by signal: {s}")
        } else {
            write!(f, "exit status: unknown ({})", self.0)
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitStatusError(ExitStatus);

impl Into<ExitStatus> for ExitStatusError {
    fn into(self) -> ExitStatus {
        self.0
    }
}

impl ExitStatusError {
    pub fn code(self) -> Option<NonZeroI32> {
        NonZeroI32::new(self.0.code().unwrap_or(-1))
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitCode(i32);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);

    pub fn as_i32(&self) -> i32 {
        self.0
    }
}

impl From<u8> for ExitCode {
    fn from(code: u8) -> Self {
        Self(code as i32)
    }
}

// the child

pub struct Process {
    pid: u64,
    /// Once reaped, kept: a second `wait` answers the same, and the pid may
    /// already be someone else's
    status: Option<ExitStatus>,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    pub fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "invalid argument: can't kill an exited process",
            ));
        }
        cvt(abi::miku_kill(self.pid, 9))?; // SIGKILL
        Ok(())
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(s) = self.status {
            return Ok(s);
        }
        let mut raw: i64 = 0;
        cvt(abi::miku_wait4(self.pid, &mut raw, 0))?;
        let s = ExitStatus(raw as i32);
        self.status = Some(s);
        Ok(s)
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(s) = self.status {
            return Ok(Some(s));
        }
        let mut raw: i64 = 0;
        // WNOHANG = 1
        let r = abi::miku_wait4(self.pid, &mut raw, 1);
        if r == 0 {
            return Ok(None); // still running
        }
        cvt(r)?;
        let s = ExitStatus(raw as i32);
        self.status = Some(s);
        Ok(Some(s))
    }
}

pub struct CommandArgs<'a> {
    iter: crate::slice::Iter<'a, crate::ffi::OsString>,
}

impl<'a> Iterator for CommandArgs<'a> {
    type Item = &'a OsStr;
    fn next(&mut self) -> Option<&'a OsStr> {
        self.iter.next().map(|a| a.as_os_str())
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for CommandArgs<'a> {
    fn len(&self) -> usize {
        self.iter.len()
    }
    fn is_empty(&self) -> bool {
        self.iter.is_empty()
    }
}

impl<'a> fmt::Debug for CommandArgs<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter.clone()).finish()
    }
}

pub type ChildPipe = crate::sys::pipe::Pipe;

/// Drain a child's stdout and stderr, one after the other.
///
/// A pipe is a 64 KiB ring (`kernel/fs/vfs/pipe.rs`), so a child writing more
/// than that to stderr while stdout stays open blocks, and so does this
pub fn read_output(
    out: ChildPipe,
    stdout: &mut Vec<u8>,
    err: ChildPipe,
    stderr: &mut Vec<u8>,
) -> io::Result<()> {
    out.read_to_end(stdout)?;
    err.read_to_end(stderr)?;
    Ok(())
}

pub fn getpid() -> u32 {
    abi::miku_getpid() as u32
}
