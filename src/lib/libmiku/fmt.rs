core::arch::global_asm!(
    ".global miku_printf",
    "miku_printf:",
    "push rbp",
    "mov rbp, rsp",
    "and rsp, -16",
    "sub rsp, 48",
    "mov [rsp], rsi",
    "mov [rsp+8], rdx",
    "mov [rsp+16], rcx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "mov rsi, rsp",
    "call miku_printf_impl",
    "mov rsp, rbp",
    "pop rbp",
    "ret",
);

core::arch::global_asm!(
    ".global miku_snprintf",
    "miku_snprintf:",
    "push rbp",
    "mov rbp, rsp",
    "and rsp, -16",
    "sub rsp, 48",
    "mov [rsp], rcx",
    "mov [rsp+8], r8",
    "mov [rsp+16], r9",
    "mov rcx, rsp",
    "call miku_snprintf_impl",
    "mov rsp, rbp",
    "pop rbp",
    "ret",
);

// dprintf(fd, fmt, ...) - printf to arbitrary file descriptor
// args: rdi=fd, rsi=fmt, rdx..r9=varargs
core::arch::global_asm!(
    ".global miku_dprintf",
    "miku_dprintf:",
    "push rbp",
    "mov rbp, rsp",
    "and rsp, -16",
    "sub rsp, 48",
    "mov [rsp], rdx",
    "mov [rsp+8], rcx",
    "mov [rsp+16], r8",
    "mov [rsp+24], r9",
    "mov rdx, rsp",
    "call miku_dprintf_impl",
    "mov rsp, rbp",
    "pop rbp",
    "ret",
);

// fprintf(fd, fmt, ...) - alias for dprintf
core::arch::global_asm!(
    ".global miku_fprintf",
    "miku_fprintf:",
    "jmp miku_dprintf",
);

struct FmtSpec {
    width: usize,
    pad_char: u8,
    left_align: bool,
    is_long: bool,
    is_longlong: bool,
    is_size: bool,
}

impl FmtSpec {
    fn new() -> Self {
        Self {
            width: 0,
            pad_char: b' ',
            left_align: false,
            is_long: false,
            is_longlong: false,
            is_size: false,
        }
    }
}

unsafe fn read_arg(args: *const u64, idx: usize) -> u64 {
    *args.add(idx)
}

fn parse_spec(fmt: *const u8, start: usize) -> (FmtSpec, usize) {
    let mut spec = FmtSpec::new();
    let mut i = start;

    unsafe {
        if *fmt.add(i) == b'-' { spec.left_align = true; i += 1; }
        if *fmt.add(i) == b'0' && !spec.left_align { spec.pad_char = b'0'; i += 1; }

        while *fmt.add(i) >= b'0' && *fmt.add(i) <= b'9' {
            spec.width = spec.width * 10 + (*fmt.add(i) - b'0') as usize;
            i += 1;
        }

        if *fmt.add(i) == b'l' {
            i += 1;
            spec.is_long = true;
            if *fmt.add(i) == b'l' { i += 1; spec.is_longlong = true; }
        } else if *fmt.add(i) == b'z' {
            i += 1;
            spec.is_size = true;
        }
    }

    (spec, i)
}

fn pad_and_emit<E: FnMut(u8)>(emit: &mut E, data: &[u8], spec: &FmtSpec) -> i32 {
    let data_len = data.len();
    let pad_count = if spec.width > data_len { spec.width - data_len } else { 0 };
    let mut written = 0i32;

    if !spec.left_align && spec.pad_char == b' ' {
        for _ in 0..pad_count { emit(b' '); written += 1; }
    }

    let sign_and_zero_pad = !spec.left_align && spec.pad_char == b'0';
    if sign_and_zero_pad {
        if !data.is_empty() && data[0] == b'-' {
            emit(b'-');
            written += 1;
            for _ in 0..pad_count { emit(b'0'); written += 1; }
            for &b in &data[1..] { emit(b); written += 1; }
            return written;
        }
        for _ in 0..pad_count { emit(b'0'); written += 1; }
    }

    for &b in data { emit(b); written += 1; }

    if spec.left_align {
        for _ in 0..pad_count { emit(b' '); written += 1; }
    }

    written
}

#[no_mangle]
pub unsafe extern "C" fn miku_printf_impl(fmt: *const u8, args: *const u64) -> i32 {
    if fmt.is_null() { return -1; }
    let mut written: i32 = 0;
    let mut emitter = |b: u8| { crate::io::miku_write(1, &b as *const u8, 1); };
    written = format_core(fmt, args, &mut emitter);
    written
}

#[no_mangle]
pub unsafe extern "C" fn miku_snprintf_impl(buf: *mut u8, max: usize, fmt: *const u8, args: *const u64) -> i32 {
    if buf.is_null() || max == 0 || fmt.is_null() { return 0; }
    let limit = max - 1;
    let mut out = 0usize;
    let mut emitter = |b: u8| {
        if out < limit { *buf.add(out) = b; out += 1; }
    };
    let written = format_core(fmt, args, &mut emitter);
    *buf.add(out) = 0;
    written
}

#[no_mangle]
pub unsafe extern "C" fn miku_dprintf_impl(fd: u64, fmt: *const u8, args: *const u64) -> i32 {
    if fmt.is_null() { return -1; }
    let mut emitter = |b: u8| { crate::io::miku_write(fd, &b as *const u8, 1); };
    format_core(fmt, args, &mut emitter)
}

unsafe fn format_core<E: FnMut(u8)>(fmt: *const u8, args: *const u64, emit: &mut E) -> i32 {
    let mut written: i32 = 0;
    let mut i = 0usize;
    let mut ai = 0usize;
    let mut num_buf = [0u8; 24];

    loop {
        let c = *fmt.add(i);
        if c == 0 { break; }

        if c != b'%' {
            emit(c);
            written += 1;
            i += 1;
            continue;
        }

        i += 1;
        let (spec, new_i) = parse_spec(fmt, i);
        i = new_i;

        let conv = *fmt.add(i);
        if conv == 0 { break; }

        match conv {
            b's' => {
                let s = read_arg(args, ai) as *const u8;
                ai += 1;
                if !s.is_null() {
                    let len = crate::string::miku_strlen(s);
                    let slice = core::slice::from_raw_parts(s, len);
                    written += pad_and_emit(emit, slice, &spec);
                }
            }
            b'd' | b'i' => {
                let v = if spec.is_longlong || spec.is_long || spec.is_size {
                    read_arg(args, ai) as i64
                } else {
                    read_arg(args, ai) as i32 as i64
                };
                ai += 1;
                crate::num::miku_itoa(v, num_buf.as_mut_ptr());
                let len = crate::string::miku_strlen(num_buf.as_ptr());
                written += pad_and_emit(emit, &num_buf[..len], &spec);
            }
            b'u' => {
                let v = if spec.is_longlong || spec.is_long || spec.is_size {
                    read_arg(args, ai)
                } else {
                    read_arg(args, ai) as u32 as u64
                };
                ai += 1;
                crate::num::miku_utoa(v, num_buf.as_mut_ptr());
                let len = crate::string::miku_strlen(num_buf.as_ptr());
                written += pad_and_emit(emit, &num_buf[..len], &spec);
            }
            b'x' => {
                let v = if spec.is_longlong || spec.is_long || spec.is_size {
                    read_arg(args, ai)
                } else {
                    read_arg(args, ai) as u32 as u64
                };
                ai += 1;
                let len = crate::num::utoa_hex(v, num_buf.as_mut_ptr());
                written += pad_and_emit(emit, &num_buf[..len], &spec);
            }
            b'X' => {
                let v = if spec.is_longlong || spec.is_long || spec.is_size {
                    read_arg(args, ai)
                } else {
                    read_arg(args, ai) as u32 as u64
                };
                ai += 1;
                let len = crate::num::utoa_hex_upper(v, num_buf.as_mut_ptr());
                written += pad_and_emit(emit, &num_buf[..len], &spec);
            }
            b'o' => {
                let v = if spec.is_longlong || spec.is_long || spec.is_size {
                    read_arg(args, ai)
                } else {
                    read_arg(args, ai) as u32 as u64
                };
                ai += 1;
                let len = crate::num::utoa_oct(v, num_buf.as_mut_ptr());
                written += pad_and_emit(emit, &num_buf[..len], &spec);
            }
            b'c' => {
                let ch = read_arg(args, ai) as u8;
                ai += 1;
                let byte_arr = [ch];
                written += pad_and_emit(emit, &byte_arr, &spec);
            }
            b'p' => {
                let v = read_arg(args, ai);
                ai += 1;
                num_buf[0] = b'0';
                num_buf[1] = b'x';
                let hex_len = crate::num::utoa_hex(v, num_buf.as_mut_ptr().add(2));
                let total_len = 2 + hex_len;
                written += pad_and_emit(emit, &num_buf[..total_len], &spec);
            }
            b'%' => {
                emit(b'%');
                written += 1;
            }
            _ => {
                emit(b'%');
                emit(conv);
                written += 2;
            }
        }
        i += 1;
    }
    written
}
