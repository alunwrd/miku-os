// aosh: QML desktop shell

mod anim;
mod check;
mod input;
mod io;
mod ipc;
mod js;
mod mayland;
mod model;
mod object;
mod qml;
mod scene;
mod services;
mod shell;
mod shot;
mod types;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        // `aosh [file.qml]` runs a file or the user's shell.qml, `aosh -c NAME`
        // runs a named config
        None => std::process::exit(shell::run(None, None)),
        Some(f) if f.ends_with(".qml") => std::process::exit(shell::run(Some(std::path::Path::new(f)), None)),
        Some("-c") => match args.get(2) {
            Some(name) if shell::config_name(name) => std::process::exit(shell::run(None, Some(name))),
            _ => {
                eprintln!("aosh: -c wants the name of a config: ~/.config/aosh/NAME, else /etc/aosh/NAME");
                std::process::exit(2);
            }
        },
        Some("--eval") => {
            let Some(source) = args.get(2) else {
                eprintln!("aosh: --eval wants the JavaScript to run");
                std::process::exit(2);
            };
            let js = match js::Js::new() {
                Ok(js) => js,
                Err(e) => {
                    eprintln!("aosh: {e}");
                    std::process::exit(1);
                }
            };
            match js.eval(source) {
                Ok(value) => println!("{value}"),
                Err(e) => {
                    eprintln!("aosh: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("check") => std::process::exit(check::run(&args[2..])),
        Some("shot") => std::process::exit(shot::run(&args[2..])),
        Some("ipc") => std::process::exit(ipc::socket::client(&args[2..])),
        Some("run") => std::process::exit(run(args.get(2).map(String::as_str))),
        Some("eval") => {
            let (Some(file), Some(expr)) = (args.get(2), args.get(3)) else {
                eprintln!("usage: aosh eval <file.qml> '<expression>'");
                std::process::exit(2);
            };
            std::process::exit(eval(file, expr));
        }
        _ => {
            eprintln!("usage: aosh [file.qml]   the shell: the config ~/.config/aosh/default or /etc/aosh/default names,");
            eprintln!("                         else ~/.config/aosh/shell.qml, else /etc/aosh/shell.qml");
            eprintln!("       aosh -c NAME   a config's shell: ~/.config/aosh/NAME, else /etc/aosh/NAME");
            eprintln!("       aosh --eval '<javascript>'");
            eprintln!("       aosh check <file or directory>...");
            eprintln!("       aosh ipc [-c NAME] call <target> <function> [args...]   (a running shell's IpcHandler)");
            eprintln!("       aosh ipc [-c NAME] show");
            eprintln!("       aosh run [file.qml]   (no window: timers and console.log)");
            eprintln!("       aosh eval <file.qml> '<expression>'   (in the scope of its root object)");
            eprintln!("       aosh shot <file.qml> [--home D] [--map FROM=TO] [--size WxH] [--time ISO] [--settle ms] [--out x.png] [--geometry x.json] [--profile] [--against ref.png --crops DIR [--ignore-top N]]");
            std::process::exit(2);
        }
    }
}

/// Loads a shell file with no window and runs its timers on the real clock
/// until none remain
fn run(path: Option<&str>) -> i32 {
    let path = std::path::Path::new(path.unwrap_or("/etc/aosh/shell.qml"));
    let mut engine = match object::engine::Engine::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("aosh: {e}");
            return 1;
        }
    };
    if let Err(e) = engine.load_file(path) {
        eprintln!("aosh: {e}");
        return 1;
    }
    // Runs until no timer is due, nothing animates, and no process is running
    let start = std::time::Instant::now();
    loop {
        let now = start.elapsed().as_millis() as u64;
        engine.advance(now);
        let due = engine.next_due();
        let animating = engine.animating();
        if due.is_none() && !animating && !engine.running_processes() {
            return 0;
        }
        let mut wait = due.map(|d| d.saturating_sub(start.elapsed().as_millis() as u64)).unwrap_or(1000);
        if animating {
            wait = wait.min(16);
        }
        let fds = engine.io_fds();
        let mut p: Vec<libc::pollfd> = fds.iter().map(|&(fd, _)| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
        // A process that exits with nothing to say is noticed within this
        let wait = if engine.running_processes() { wait.min(50) } else { wait };
        // SAFETY: pollfd array of the given length
        unsafe { libc::poll(p.as_mut_ptr(), p.len() as libc::nfds_t, wait as i32) };
        for (pf, &(_, f)) in p.iter().zip(fds.iter()) {
            if pf.revents != 0 {
                engine.io_ready(f);
            }
        }
    }
}

/// Loads a file and evaluates an expression in the scope of its root. Load
/// errors are printed first
fn eval(file: &str, expr: &str) -> i32 {
    let mut engine = match object::engine::Engine::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("aosh: {e}");
            return 1;
        }
    };
    let root = match engine.load_file(std::path::Path::new(file)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("aosh: {e}");
            return 1;
        }
    };
    let problems = engine.take_log().len();
    match engine.eval(root, expr) {
        Ok(v) => {
            println!("{v}");
            if problems > 0 {
                eprintln!("aosh: {problems} problem(s) loading {file}, above");
            }
            0
        }
        Err(e) => {
            eprintln!("aosh: {e}");
            1
        }
    }
}

/// Writes a line to the kernel log on MikuOS (read with `dmesg`), or to stderr
/// on the host
pub fn say(line: &str) {
    #[cfg(target_os = "miku")]
    {
        extern "C" {
            fn miku_klog(buf: *const u8, len: usize) -> i64;
        }
        let line = format!("aosh: {line}\n");
        // SAFETY: libmiku entry; reads `len` bytes from `buf`
        unsafe { miku_klog(line.as_ptr(), line.len()) };
    }
    #[cfg(not(target_os = "miku"))]
    eprintln!("aosh: {line}");
}

