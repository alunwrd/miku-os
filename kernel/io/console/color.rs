#[macro_export]
macro_rules! cprint {
    ($r:expr, $g:expr, $b:expr, $($arg:tt)*) => {{
        $crate::io::console::print_colored($r, $g, $b, format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! cprintln {
    ($r:expr, $g:expr, $b:expr, $($arg:tt)*) => {{
        $crate::io::console::print_colored($r, $g, $b, format_args!("{}\n", format_args!($($arg)*)));
    }};
}

#[macro_export]
macro_rules! print_error {
    ($($arg:tt)*) => {
        $crate::cprintln!(255, 50, 50, $($arg)*)
    };
}

#[macro_export]
macro_rules! print_success {
    ($($arg:tt)*) => {
        $crate::cprintln!(100, 220, 150, $($arg)*)
    };
}

#[macro_export]
macro_rules! print_info {
    ($($arg:tt)*) => {
        $crate::cprintln!(128, 222, 217, $($arg)*)
    };
}

#[macro_export]
macro_rules! print_warn {
    ($($arg:tt)*) => {
        $crate::cprintln!(220, 220, 100, $($arg)*)
    };
}
