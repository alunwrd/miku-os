// Starts a program without becoming it
//
// `fork` then `exec`: a window manager that exec'd would hand over `may_wm`,
// and a launcher would hand over its exclusive keyboard. Turns one string into
// an argv without an allocator.

/// The most arguments one command may carry
pub const MAX_ARGS: usize = 8;

/// Why a command did not start. Returned, since the window manager writes to
/// the desktop's complaint channel and the launcher to its window
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Failed {
    /// Nothing but spaces
    Empty,
    /// Longer than a config line can carry
    TooLong,
    /// More words than `MAX_ARGS`
    TooManyArgs,
    /// The kernel refused to fork
    NoFork,
}

impl Failed {
    pub fn as_str(self) -> &'static str {
        match self {
            Failed::Empty => "there is no command there",
            Failed::TooLong => "that command is too long for the config to carry",
            Failed::TooManyArgs => "more arguments than a command may carry",
            Failed::NoFork => "could not fork",
        }
    }
}

/// Starts `cmd` in a child with none of the desktop's authority. Returns the
/// pid. See `run_with` for the other case
#[cfg(feature = "userspace")]
pub fn run(cmd: &str) -> Result<u64, Failed> {
    run_with(cmd, false)
}

/// The same, saying whether the child keeps `CAP_MAY_PRIV`. `keep_priv` is true
/// only for `maysession`, starting the programs that bind a privileged global:
/// the window manager, bar, and terminal.
///
/// The words go into one buffer with spaces turned into terminators, and argv
/// points into it. Offsets are recorded first and pointers taken after the
/// buffer is complete.
#[cfg(feature = "userspace")]
pub fn run_with(cmd: &str, keep_priv: bool) -> Result<u64, Failed> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Err(Failed::Empty);
    }
    // One byte longer than the config carries, for the last terminator
    let mut buf = [0u8; crate::conf::MAX_TEXT + 1];
    if cmd.len() >= buf.len() {
        return Err(Failed::TooLong);
    }
    let mut at = [0usize; MAX_ARGS];
    let mut n = 0;
    let mut w = 0usize;
    for word in cmd.split_whitespace() {
        if n == MAX_ARGS {
            return Err(Failed::TooManyArgs);
        }
        at[n] = w;
        buf[w..w + word.len()].copy_from_slice(word.as_bytes());
        w += word.len();
        buf[w] = 0;
        w += 1;
        n += 1;
    }
    if n == 0 {
        return Err(Failed::Empty);
    }
    let mut argv = [core::ptr::null::<u8>(); MAX_ARGS];
    for i in 0..n {
        argv[i] = unsafe { buf.as_ptr().add(at[i]) };
    }
    unsafe {
        let pid = miku::miku_fork();
        if pid == 0 {
            // A launched program is not the window manager. fork copies the
            // credential, and the session carries CAP_MAY_PRIV to bind may_wm.
            // A program inheriting it could bind may_console or may_layer and
            // put a surface over everything while reading the keyboard.
            //
            // Dropped from the bounding set too, so nothing the child starts
            // regains it. Errors are ignored: a build with no capabilities to
            // drop still starts the program
            if !keep_priv {
                let keep = !miku::cred::cap_bit(miku::cred::CAP_MAY_PRIV);
                let _ = miku::cred::keep_only_caps(keep);
            }

            // This process's environment, the session's: what `mikudod` built
            // for the login, handed down through `maysession` and the window
            // manager
            let envp = miku::miku_environ() as *const *const u8;
            miku::miku_execve(argv[0], argv.as_ptr(), n, envp, miku::miku_env_count());
            // Reached only when exec refused
            miku::exit(1);
        }
        if pid < 0 {
            return Err(Failed::NoFork);
        }
        Ok(pid as u64)
    }
}
