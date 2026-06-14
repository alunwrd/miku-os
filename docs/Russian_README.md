<div align="center">

# Miku OS

**Экспериментальная ОС на Rust**

*Работает на Rust и одном разработчике :D*

<img src="https://raw.githubusercontent.com/alunwrd/miku-os/main/docs/miku.png" width="220" alt="Miku Logo">

[![Language](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Architecture](https://img.shields.io/badge/arch-x86__64-blue.svg)]()
[![Status](https://img.shields.io/badge/status-release-green.svg)]()
[![License](https://img.shields.io/badge/license-MIT-lightgrey.svg)]()

</div>

---

> **Документация:** [Russian](Russian_README.md) | [English](English_README.md) | [Japanese](Japanese_README.md)

---

## О проекте

**Miku OS** это операционная система, разработанная с нуля в `no_std` окружении.
Не использует стандартную библиотеку (`libc`), полностью контролирует железо и архитектуру памяти.
ELF динамическая линковка, разделяемые библиотеки, userspace процессы, init-демон (mikuD)
и управление процессами (fork/exec/wait) реализованы с нуля.

> Весь код написан на Rust. Ассемблер используется только для загрузчика, обработчика syscall и переключения контекста.

---

## Технические характеристики

### Ядро

| Компонент | Описание |
|:--|:--|
| **Архитектура** | x86_64, `#![no_std]`, `#![no_main]` |
| **Загрузчик** | GRUB2 + Multiboot2, фреймбуфер (BGR/RGB автоопределение) |
| **Защита** | GDT + TSS + IST (double fault, page fault, GPF), ring 0 / ring 3 |
| **Прерывания** | IDT: таймер, клавиатура, page fault, GPF, #UD, #NM, double fault |
| **PIC** | PIC8259 (смещение 32/40) |
| **SSE** | CR0.EM=0, CR0.MP=1, CR4.OSFXSR=1, CR4.OSXMMEXCPT=1 |
| **Куча** | 32 MB, linked list аллокатор |
| **Syscall** | SYSCALL/SYSRET через MSR, naked asm обработчик, сохранение R8/R9/R10 (модульный: syscall/) |
| **Сигналы** | SIGKILL (9), SIGTERM (15), SIGCHLD (17), 32-бит битовая маска |
| **Init** | mikuD (PID 1) - systemd-подобный менеджер сервисов |
| **ACPI** | Парсер RSDP/RSDT/XSDT, перечисление MADT (LAPIC + IOAPIC) |
| **APIC** | Local APIC + I/O APIC драйвер (заменяет PIC8259) |
| **SMP** | Запуск нескольких ядер: AP трамплин, per-CPU состояние (percpu), последовательность SIPI |
| **PS/2** | Инициализация контроллера клавиатуры |
| **USB** | USB legacy handoff (освобождение EHCI/xHCI от BIOS) |
| **Splash** | Загрузочный экран через фреймбуфер |
| **fwload** | Загрузчик прошивок по требованию из `/lib/firmware` (модель Linux `request_firmware`) |

---

### mikuD - Init-демон

<details>
<summary><b>Развернуть</b></summary>

#### Обзор

mikuD это init-демон (PID 1) для MikuOS - полноценный systemd-подобный супервизор сервисов с Unix-style границами. Управляет жизненным циклом сервисов, разрешением зависимостей, таргетами (runlevels), watchdog, уведомлениями, socket activation, таймерами, запуском ELF-бинарников (ExecStart) и graceful shutdown с глобальным таймаутом.

#### Таргеты (уровни работы)

| Таргет | Значение | Описание |
|:--|:--:|:--|
| **SysInit** | 0 | Инициализация системы |
| **MultiUser** | 1 | Многопользовательский режим (по умолчанию) |
| **Graphical** | 2 | Графический режим |
| **Rescue** | 3 | Режим восстановления / однопользовательский |

Сервисы активируются когда таргет >= их объявленного таргета. Переключение таргетов запускает автоматическую сверку.

#### Типы сервисов

| Тип | Описание |
|:--|:--|
| **Simple** | Долгоживущий сервис (по умолчанию) |
| **Oneshot** | Выполнить один раз, затем пометить завершенным |
| **Notify** | Сервис сообщает о готовности через `notify_ready()` |
| **Forking** | Сервис форкает дочерний процесс |

#### Политики перезапуска

| Политика | Поведение |
|:--|:--|
| **Always** | Перезапуск при любом завершении |
| **Never** | Не перезапускать |
| **OnFailure** | Перезапуск только если exit code != 0 |
| **OnSuccess** | Перезапуск только если exit code == 0 |
| **OnAbnormal** | Перезапуск при сигнале или ненулевом выходе |

#### Типы зависимостей

| Тип | Поведение |
|:--|:--|
| **Requires** (deps) | Жесткая зависимость - сервис падает если зависимость падает |
| **Wants** | Мягкая зависимость - сервис продолжает если зависимость падает |
| **Conflicts** | Остановить конфликтующий сервис перед запуском |

#### Возможности

| Возможность | Детали |
|:--|:--|
| **ExecStart** | Запуск ELF-бинарников с диска как сервисов |
| **Watchdog** | Сервис должен пинговать в пределах таймаута, иначе перезапуск |
| **Notify** | Аналог sd_notify - сервис сигнализирует о готовности |
| **Условия** | ConditionPathExists, ConditionServiceActive, ConditionTargetActive |
| **Маскирование** | Полный запрет запуска сервиса |
| **Critical** | Защищенные сервисы не могут быть остановлены пользователем |
| **Защита от burst** | Максимум 5 перезапусков за 10 секунд |
| **Graceful shutdown** | Сначала некритичные, затем критичные, 30 сек глобальный таймаут |
| **Анализ загрузки** | Данные о времени запуска всех сервисов |
| **Переменные окружения** | До 8 пар key=value на сервис |
| **Таймауты** | Настраиваемые таймауты запуска/остановки (по умолчанию 10 сек) |
| **Хуки перезапуска** | Callback перед повторным запуском сервиса |
| **Isolate** | Переключение таргета с остановкой ненужных сервисов |

#### Журнал (лог событий)

128-записный кольцевой буфер для всех событий mikuD:

| Событие | Символ | Описание |
|:--|:--:|:--|
| Started | + | Сервис запущен |
| Stopped | - | Сервис остановлен |
| Exited | x | Сервис завершился (с кодом выхода) |
| Failed | ! | Сервис провалился |
| DepFailed | d | Ошибка зависимости |
| ExecFailed | E | Неудача запуска ELF-бинарника |
| Reloaded | R | SIGHUP перезагрузка |
| WatchdogTimeout | W | Истечение watchdog |
| BurstLimit | B | Достигнут лимит перезапусков |
| Shutdown | S | Инициирован graceful shutdown |
| TimerFired | F | Срабатывание таймера |
| SocketActivated | A | Срабатывание socket activation |

События имеют уровни серьезности: info (0), notice (1), warning (2), critical (3).

#### Таймеры

| Тип | Поведение |
|:--|:--|
| **Interval** | Срабатывание каждые N тиков |
| **Oneshot** | Одноразовое срабатывание через N тиков |
| **Realtime** | Срабатывание каждые N тиков относительно boot time |

Максимум 16 таймеров. Таймеры запускают сервис при срабатывании.

#### Socket Activation

Сервисы могут запускаться по требованию при поступлении соединения на зарегистрированный порт.
Поддержка Stream (TCP) и Dgram (UDP). Максимум 16 сокетов.

#### Unit-файлы (.service)

INI-формат, загрузка из `/etc/mikud/`:

```ini
[Unit]
Description=My service
After=kbd network
Wants=logging
Conflicts=rescue-shell
ConditionPathExists=/etc/config

[Service]
Type=simple
ExecStart=/usr/bin/myservice
Restart=always
RestartSec=50
Priority=5
WatchdogSec=100
TimeoutStartSec=2500
RemainAfterExit=false
Critical=false
Environment=LANG=en

[Install]
WantedBy=multi-user
```

#### Команды оболочки (sv)

| Команда | Описание |
|:--|:--|
| `sv list` | Список всех сервисов (состояние, PID, перезапуски) |
| `sv status <name>` | Детальный статус + записи журнала |
| `sv start <name>` | Запуск сервиса |
| `sv stop <name>` | Остановка сервиса (graceful) |
| `sv restart <name>` | Перезапуск сервиса |
| `sv reload <name>` | Отправка SIGHUP для перезагрузки конфигурации |
| `sv enable <name>` | Включение сервиса |
| `sv disable <name>` | Выключение сервиса (остановка + деактивация) |
| `sv mask <name>` | Запрет запуска сервиса |
| `sv unmask <name>` | Разрешение запуска замаскированного сервиса |
| `sv force-stop <name>` | Принудительное завершение (даже critical) |
| `sv journal [name]` | Лог событий (последние 20 или по сервису) |
| `sv target [name]` | Показ/установка активного таргета |
| `sv isolate <tgt>` | Переключение таргета, остановка лишних |
| `sv analyze` | Анализ времени загрузки |
| `sv tree <name>` | Дерево зависимостей |
| `sv rdeps <name>` | Обратные зависимости |
| `sv cat <name>` | Показ unit-конфигурации сервиса |
| `sv load <path>` | Загрузка .service unit-файла |
| `sv scan` | Сканирование /etc/mikud/ |
| `sv timer list` | Список таймеров |
| `sv timer start/stop <name>` | Управление таймерами |

</details>

---

### ELF загрузчик и динамическая линковка

<details>
<summary><b>ELF загрузчик</b></summary>

#### Возможности

| Возможность | Описание |
|:--|:--|
| **Форматы** | ET_EXEC (статический), ET_DYN (PIE) |
| **Сегменты** | PT_LOAD, PT_INTERP, PT_DYNAMIC, PT_TLS, PT_GNU_RELRO, PT_GNU_STACK |
| **Релокации** | R_X86_64_RELATIVE, R_X86_64_JUMP_SLOT, R_X86_64_GLOB_DAT, R_X86_64_64 |
| **Безопасность** | W^X enforcement (запрет W+X сегментов), RELRO |
| **ASLR** | 20-бит энтропия для PIE (RDRAND + TSC fallback) |
| **Стек** | SysV ABI: argc, argv, envp, auxv (16-байт выравнивание) |
| **TLS** | Thread Local Storage (через FS.base регистр) |

#### Модульная структура

| Модуль | Описание |
|:--|:--|
| **elf_loader.rs** | Парсинг ELF, маппинг сегментов |
| **exec_elf.rs** | Создание процесса, построение стека |
| **dynlink.rs** | Динамическая линковка (делегирует в reloc.rs) |
| **reloc.rs** | Унифицированный движок релокаций |
| **vfs_read.rs** | Унифицированное чтение файлов (VFS + ext2) |
| **random.rs** | RDRAND/TSC случайные числа, ASLR |

#### Записи auxv

| Ключ | Описание |
|:--|:--|
| AT_PHDR | Виртуальный адрес заголовков программы |
| AT_PHENT | Размер записи заголовка |
| AT_PHNUM | Количество заголовков |
| AT_PAGESZ | Размер страницы (4096) |
| AT_ENTRY | Точка входа исполняемого файла |
| AT_BASE | Базовый адрес интерпретатора |
| AT_RANDOM | 16 байт случайных данных |

</details>

<details>
<summary><b>ld-miku (динамический линкер)</b></summary>

#### Обзор

`ld-miku` это ELF динамический линкер для MikuOS. Написан на Rust в `#![no_std]` окружении,
компилируется как статический PIE бинарь.

#### Процесс загрузки

```
1. Ядро загружает ELF -> обнаруживает PT_INTERP
2. ld-miku.so маппится из INCLUDE_BYTES в память
3. ld-miku запускается -> парсит auxv (AT_PHDR/AT_ENTRY)
4. Определяет необходимые библиотеки из DT_NEEDED
5. Маппит разделяемые библиотеки через SYS_MAP_LIB syscall
6. Применяет PLT/GOT релокации
7. Экспортирует символы в глобальную таблицу
8. Выполняет DT_INIT / DT_INIT_ARRAY
9. Прыжок на точку входа исполняемого файла
```

#### Особенности

- Глобальная таблица символов (до 1024 символов)
- Разрешение weak символов
- Рекурсивная загрузка зависимостей (до 16 библиотек)
- Поддержка R_X86_64_COPY релокаций
- DT_HASH / DT_GNU_HASH для точного подсчета символов
- Корректный пропуск envp при парсинге auxv

</details>

<details>
<summary><b>Разделяемые библиотеки (solib)</b></summary>

#### Глобальный кэш библиотек

| Параметр | Значение |
|:--|:--|
| **Макс. кэш** | 32 библиотеки |
| **Пути поиска** | /lib, /usr/lib |
| **Маппинг страниц** | Все сегменты копируются для каждого процесса |
| **OOM защита** | Прерывание parse_and_prepare при OOM без кэширования битых данных |

#### SYS_MAP_LIB syscall (nr=15)

Ядро парсит ELF сегменты и маппит разделяемую библиотеку напрямую в адресное пространство процесса.

- Read-only сегменты -> приватная копия из кэша
- Writable сегменты -> новая аллокация для каждого процесса
- Откат при неудаче map_page

#### Системные библиотеки

`libmiku.so` встроена в ядро через `include_bytes!` и регистрируется в кэше при старте через `solib::preload`.

#### Команды оболочки

| Команда | Описание |
|:--|:--|
| `ldconfig` | Сканирование /lib и /usr/lib, обновление кэша |
| `ldd` | Список кэшированных библиотек |

</details>

---

### libmiku.so (стандартная библиотека)

<details>
<summary><b>Развернуть</b></summary>

#### Обзор

libmiku это C-совместимая стандартная библиотека для MikuOS. Написана на Rust, 63 модуля, 956 экспортируемые функции.
Загружается динамически через ld-miku, используется всеми userspace программами.
Включает POSIX libc-совместимый слой (stdio, stdlib, string.h и т.д.).

#### Категории модулей

| Категория | Модули |
|:--|:--|
| **Структуры данных** | vec, list, hashmap, treemap, trie, queue, ringbuf, ringbuf2, heap_queue, bitset, channel |
| **Строки** | string, strbuf, ctype, utf8, format, regex, glob |
| **I/O** | io, bufio, stdio, file, dir, path |
| **Числа / математика** | num, math, random, convert, endian, bitops |
| **Кодирование** | base64, hex, json, csv, ini, lz |
| **Хэш / криптография** | sha256, checksum, hash, uuid |
| **Система** | sys, proc, signal, env, errno, args, getopt |
| **Параллелизм** | sync, channel, event, timer |
| **Время** | time, datetime |
| **Память** | mem, heap, arena, pool, slab |
| **Логирование / тесты** | log, test, panic |
| **Сортировка** | sort |
| **libc-совместимость** | libc (fopen/fclose/fread/fwrite/fprintf/fgets/fputs и др., 151 функция) |

> Прежний модуль `util` разбит на три: `math` (abs/min/max/clamp/isqrt/div_ceil/is_prime), `random` (srand/rand/rand_range/rand_bytes) и `panic` (assert_fail/panic/assert_eq/assert_not_null). Имена символов не менялись, ABI-совместимость сохранена.

#### Модуль: io (ввод/вывод)

| Функция | Описание |
|:--|:--|
| `miku_write(fd, buf, len)` | Запись в fd |
| `miku_read(fd, buf, len)` | Чтение из fd |
| `miku_print(str)` | Вывод строки |
| `miku_println(str)` | Вывод строки + перенос |
| `miku_puts(str)` | Совместимость с puts |
| `miku_putchar(c)` | Вывод 1 байта |
| `miku_getchar()` | Ввод 1 байта |
| `miku_readline(buf, max)` | Ввод строки (фикс. буфер) |
| `miku_getline()` | Ввод строки (malloc, нужен free) |

#### Модуль: string (строки)

| Функция | Описание |
|:--|:--|
| `miku_strlen` | Длина строки |
| `miku_strcmp` / `miku_strncmp` | Сравнение строк |
| `miku_strcpy` / `miku_strncpy` | Копирование строк |
| `miku_strcat` / `miku_strncat` | Конкатенация строк |
| `miku_strchr` / `miku_strrchr` | Поиск символа |
| `miku_strstr` | Поиск подстроки |
| `miku_strdup` | Дублирование строки (malloc) |
| `miku_toupper` / `miku_tolower` | Преобразование регистра |
| `miku_isdigit` / `miku_isalpha` / `miku_isalnum` / `miku_isspace` | Классификация символов |
| `miku_strtok` | Токенизация (stateful) |
| `miku_strpbrk` | Поиск набора символов |
| `miku_strspn` / `miku_strcspn` | Длина префикса |
| `miku_strtol` / `miku_strtoul` | Строка в число (base 0/8/10/16) |
| `miku_strlcpy` / `miku_strlcat` | BSD безопасные копирование/конкатенация |

#### Модуль: num (числа)

| Функция | Описание |
|:--|:--|
| `miku_itoa(val, buf)` | Целое в строку |
| `miku_utoa(val, buf)` | Беззнаковое в строку |
| `miku_atoi(str)` | Строка в целое |
| `miku_print_int(val)` | Вывод десятичного |
| `miku_print_hex(val)` | Вывод 0x... |

#### Модуль: mem (память)

| Функция | Описание |
|:--|:--|
| `miku_memset` | Заполнение памяти |
| `miku_memcpy` | Копирование памяти |
| `miku_memmove` | Копирование (с перекрытием) |
| `miku_memcmp` | Сравнение |
| `miku_bzero` | Обнуление |
| `miku_memchr` | Поиск байта |
| `miku_memrchr` | Обратный поиск байта |
| `miku_memmem` | Поиск последовательности байт |

#### Модуль: heap (динамическая память)

| Функция | Описание |
|:--|:--|
| `miku_malloc(size)` | Выделение памяти |
| `miku_free(ptr)` | Освобождение |
| `miku_realloc(ptr, size)` | Изменение размера |
| `miku_calloc(count, size)` | Выделение с обнулением |

Реализация: mmap-based slab аллокатор. < 32KB из 128KB slab, >= 32KB через mmap/munmap.

#### Модуль: fmt (форматированный вывод)

| Функция | Описание |
|:--|:--|
| `miku_printf(fmt, ...)` | Форматированный вывод |
| `miku_snprintf(buf, max, fmt, ...)` | Вывод в буфер |

Форматы: `%s` `%d` `%u` `%x` `%c` `%p` `%%`

Реализация: `global_asm!` трамплин сохраняет rsi/rdx/rcx/r8/r9 на стек. Без XMM регистров, без проблем с SSE alignment. `%d/%x/%u` 32-битные (i32/u32).

#### Модуль: file (файловый I/O)

| Функция | Описание |
|:--|:--|
| `miku_open(path, len)` | Открыть файл |
| `miku_open_cstr(path)` | Открыть файл (C-строка) |
| `miku_close(fd)` | Закрыть |
| `miku_seek(fd, offset)` | Установить смещение |
| `miku_fsize(fd)` | Размер файла |
| `miku_read_file(path, &size)` | Прочитать файл целиком (malloc) |

#### Модуль: time (время)

| Функция | Описание |
|:--|:--|
| `miku_sleep(ticks)` | Сон (~4 мс/тик при 250 Hz) |
| `miku_sleep_ms(ms)` | Сон в миллисекундах |
| `miku_uptime()` | Тики с загрузки |
| `miku_uptime_ms()` | Миллисекунды с загрузки |

#### Модуль: proc (процесс)

| Функция | Описание |
|:--|:--|
| `miku_exit(code)` | Завершение процесса |
| `miku_getpid()` | Получить PID |
| `miku_getcwd(buf, size)` | Текущая директория |
| `miku_brk(addr)` | Расширение кучи (0=запрос) |
| `miku_mmap` / `miku_munmap` / `miku_mprotect` | Маппинг памяти |
| `miku_set_tls` / `miku_get_tls` | TLS регистр |
| `miku_map_lib(name, len)` | Маппинг разделяемой библиотеки |

#### Модули: math, random, panic (бывший util)

| Функция | Описание |
|:--|:--|
| `miku_abs` / `miku_min` / `miku_max` / `miku_clamp` | Числовые утилиты (saturating, без паники на INT64_MIN) |
| `miku_swap(a, b)` | Обмен значений |
| `miku_isqrt` / `miku_div_ceil` / `miku_is_prime` | Целочисленная математика (overflow-safe) |
| `miku_srand(seed)` / `miku_rand()` / `miku_rand_range(lo, hi)` | PRNG xorshift64 |
| `miku_rand_bytes` / `miku_rand_bool` / `miku_rand_uniform` | Расширенные генераторы |
| `miku_assert_fail(expr, file, line)` | Неудача assert |
| `miku_assert_eq` / `miku_assert_not_null` | Типизированные проверки |
| `miku_panic(msg)` | Паника (exit 134) |

</details>

---

### Userspace SDK

<details>
<summary><b>Развернуть</b></summary>

#### Обзор

MikuOS предоставляет Rust SDK для разработки userspace программ в `no_std` окружении.
C также поддерживается.

#### Структура SDK

```
src/lib/userspace/
├── Cargo.toml              конфигурация crate
├── build.rs                автогенерация stub libmiku.so
├── build.sh                скрипт сборки + деплоя
├── x86_64-miku-app.json    target спецификация
└── src/
    ├── miku.rs             SDK: extern привязки + безопасные обертки
    ├── hello.rs            пример Hello World
    └── test_full.rs        1617 тестов
```

#### Пример на Rust

```rust
#![no_std]
#![no_main]

#[path = "miku.rs"]
mod miku;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("Hello MikuOS!");
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(1); }
```

#### Сборка и деплой

```bash
cd ~/miku-os/src/lib/userspace
./build.sh hello        # сборка + копирование на data.img
```

#### Запуск в MikuOS

```
miku@os:/ $ ext4mount 3
miku@os:/ $ exec hello
Hello MikuOS!
```

#### Безопасные обертки (miku.rs)

| Обертка | Описание |
|:--|:--|
| `miku::print(s: &str)` | Вывод строки |
| `miku::println(s: &str)` | Вывод строки + перенос |
| `miku::exit(code)` | Завершение процесса |
| `miku::open(path) -> Result` | Открытие файла |
| `miku::read_file(path) -> Option` | Чтение файла целиком |
| `miku::sleep_ms(ms)` | Сон в миллисекундах |
| `miku::rand_range(lo, hi)` | Случайное число в диапазоне |
| `cstr!("text")` | Макрос C-строки |

#### Точка входа

Используется `_start_main`, а не `_start`. `miku.rs` содержит `global_asm!` трамплин `_start`, который делает `and rsp, -16` для SSE alignment и вызывает `_start_main`.

#### Тестовый набор

1617 тестов по следующим категориям:

| Категория | Количество |
|:--|:--|
| strings (базовые/расширенные) | 24 |
| numbers | 7 |
| memory | 4 |
| utilities | 7 |
| heap | 7 |
| process | 2 |
| printf / snprintf | 11 |
| time | 5 |
| file I/O | 3+ |
| libc-совместимость (stdio) | 1500+ |

</details>

---

### Управление памятью

<details>
<summary><b>Физическая память (PMM)</b></summary>

#### Фреймовый аллокатор

- Bitmap аллокатор: до 4M фреймов (16 GB RAM), 1 бит = 1 фрейм 4KB
- `free_hint` и `contiguous_hint` для ускорения поиска свободных фреймов
- Непрерывный alloc: N фреймов за один запрос
- Регионы: динамическая регистрация RAM из Multiboot2 memory map

#### Аварийный пул

| Параметр | Значение |
|:--|:--|
| **Размер пула** | 64 фрейма (256 KB) |
| **Назначение** | Только для swap-in в page fault обработчике |
| **Пополнение** | Timer ISR каждые 250Hz через `refill_emergency_pool_tick()` |

</details>

<details>
<summary><b>Виртуальная память (VMM)</b></summary>

- 4-уровневые таблицы страниц (PML4 -> PDP -> PD -> PT)
- HHDM: Higher Half Direct Map (`0xFFFF800000000000`)
- `mark_swapped()`: запись swap PTE при выгрузке страницы
- Поддержка маппинга ring 0 / ring 3
- Создание и уничтожение адресных пространств для процессов

</details>

<details>
<summary><b>mmap подсистема</b></summary>

| Параметр | Значение |
|:--|:--|
| **Диапазон MMAP** | 0x100000000 ~ 0x7F0000000000 |
| **Диапазон BRK** | 0x6000000000 ~ |
| **Макс. VMA** | 256 записей |
| **Функции** | mmap, munmap, mprotect, brk, file-backed mmap, msync |
| **File-backed** | `sys_mmap_file` лениво отображает файл (заполнение по page-fault); грязные MAP_SHARED страницы пишутся обратно на munmap/msync |
| **MAP_FIXED** | Unmap существующих маппингов + удаление перекрывающихся VMA |
| **Проверка VMA** | Откат при неудаче insert |

</details>

<details>
<summary><b>Swap</b></summary>

#### Обратное отображение (swap_map)

- Каждому физическому фрейму сопоставляется `(cr3, virt_addr, age, pinned)`
- Отслеживание до 512K фреймов (2 GB RAM)

#### Алгоритм вытеснения: clock sweep

```
Pass 1: поиск фреймов с age >= 3 (самые старые)
Pass 2: аварийный режим, любой unpinned фрейм
```

- `touch(phys)`: сброс age в 1 при обращении к странице
- `age_all()`: увеличение age всех фреймов по таймеру

#### Кодирование Swap PTE

```
bit 0     = 0  (PRESENT=0)
bit 1     = 1  (SWAP_MARKER)
bits 12.. = номер swap слота
Доп. проверка: номер слота != 0 (защита от false positive)
```

</details>

---

### Управление процессами

| Возможность | Описание |
|:--|:--|
| **fork()** | COW-клонирование процесса (глубокая копия таблиц страниц) |
| **exec()** | Замена образа процесса ELF-бинарником |
| **wait4()** | Ожидание дочернего процесса (блокирующее) |
| **kill()** | Отправка сигнала процессу (SIGTERM, SIGKILL, SIGCHLD) |
| **Сбор зомби** | Автоматический через mikuD и wait4 |
| **Иерархия процессов** | Отслеживание родитель-потомок через ppid |
| **Per-process идентичность** | `cwd`, `umask`, `uid`, `gid`, `euid`, `egid` - атомарно хранятся в `Process`, наследуются при `fork()`, синхронизируются в VFS-контекст при каждом syscall |

---

### Планировщик

| Параметр | Значение |
|:--|:--|
| **Алгоритм** | CFS, вытесняющий |
| **Макс. процессов** | 4096 |
| **Частота таймера** | 250 Hz (PIT) |
| **Окно CPU** | 250 тиков (1 секунда) |
| **Стек** | 512 KB на процесс |
| **Состояния** | Ready / Running / Sleeping / Blocked / Dead |
| **Реализация** | Lock-free: ISR использует только атомики |
| **Приоритет** | Шкала 1-20 с взвешенным vruntime |
| **Аффинность** | Per-process маска CPU |

---

### Системные вызовы

| Nr | Имя | Описание |
|:--:|:--|:--|
| **0** | `sys_exit` | Завершение процесса + yield |
| **1** | `sys_write` | Запись в stdout/stderr (fd 1/2) |
| **2** | `sys_read` | Чтение из stdin (fd 0) или файлового дескриптора |
| **3** | `sys_mmap` | Создание маппинга памяти |
| **4** | `sys_munmap` | Удаление маппинга памяти |
| **5** | `sys_mprotect` | Изменение атрибутов защиты памяти |
| **6** | `sys_brk` | Расширение кучи |
| **7** | `sys_getpid` | Получение PID текущего процесса |
| **8** | `sys_getcwd` | Получение текущей директории |
| **9** | `sys_set_tls` | Установка FS.base регистра (TLS) |
| **10** | `sys_get_tls` | Получение FS.base регистра |
| **11** | `sys_open` | Открытие файла (VFS + ext2) |
| **12** | `sys_close` | Закрытие файлового дескриптора |
| **13** | `sys_seek` | Установка смещения в файле |
| **14** | `sys_fsize` | Получение размера файла |
| **15** | `sys_map_lib` | Маппинг разделяемой библиотеки |
| **16** | `sys_sleep` | Сон процесса (~4мс/тик) |
| **17** | `sys_uptime` | Тики с момента загрузки |
| **18** | `sys_stat` | Информация о файле |
| **19** | `sys_fstat` | Информация по файловому дескриптору |
| **20** | `sys_mkdir` | Создание директории |
| **21** | `sys_rmdir` | Удаление директории |
| **22** | `sys_unlink` | Удаление файла |
| **23** | `sys_readdir` | Чтение записей директории |
| **24** | `sys_rename` | Переименование файла/директории |
| **25** | `sys_link` | Создание жесткой ссылки |
| **26** | `sys_chmod` | Изменение прав |
| **27** | `sys_chown` | Изменение владельца |
| **28** | `sys_dup` | Дублирование файлового дескриптора |
| **29** | `sys_dup2` | Дублирование в конкретный fd |
| **30** | `sys_truncate` | Усечение файла |
| **31** | `sys_write_file` | Запись содержимого файла |
| **32** | `sys_symlink` | Создание символической ссылки |
| **33** | `sys_readlink` | Чтение символической ссылки |
| **34** | `sys_pipe` | Создание pipe |
| **35** | `sys_chdir` | Смена директории |
| **36** | `sys_statfs` | Статистика файловой системы |
| **37** | `sys_fallocate` | Предварительное выделение места |
| **38** | `sys_getxattr` | Получение расширенного атрибута |
| **39** | `sys_setxattr` | Установка расширенного атрибута |
| **40** | `sys_utimensat` | Установка временных меток |
| **41** | `sys_fsync` | Сброс файла на диск |
| **42** | `sys_punch_hole` | Пробивка дыры в файле |
| **43** | `sys_fork` | Форк процесса |
| **44** | `sys_wait4` | Ожидание дочернего процесса |
| **45** | `sys_kill` | Отправка сигнала |
| **46** | `sys_exec` | Выполнение ELF-бинарника |
| **47** | `sys_umask` | Установка маски создания файлов (возвращает прежнюю) |
| **48** | `sys_getuid` | Получить реальный UID |
| **49** | `sys_getgid` | Получить реальный GID |
| **50** | `sys_geteuid` | Получить эффективный UID |
| **51** | `sys_getegid` | Получить эффективный GID |
| **52** | `sys_setuid` | Установить реальный UID (-EPERM если не root) |
| **53** | `sys_setgid` | Установить реальный GID (-EPERM если не root) |
| **54** | `sys_seteuid` | Установить эффективный UID (-EPERM если не root) |
| **55** | `sys_setegid` | Установить эффективный GID (-EPERM если не root) |
| **56** | `sys_socket` | Создать сокет (AF_INET/SOCK_STREAM) → fd ≥ 4096 |
| **57** | `sys_connect` | Подключить сокет к (ip, port) |
| **58** | `sys_send` | Отправить данные через сокет fd |
| **59** | `sys_recv` | Получить данные из сокета fd (0 = EOF) |

Всего: 60 syscall (0..59). Socket fd начинаются с `SOCK_FD_BASE = 4096`; `read`/`write`/`close` маршрутизируются в слой сокетов по диапазону fd. Таймер - LAPIC на 250 Гц; PIT используется только для калибровки LAPIC. Таблица FD per-process: `MikuVFS::fd_tables` это `BTreeMap<pid, FdTable>`. `fork()` клонирует таблицу родителя ребёнку; при exit таблица удаляется и каждый держимый vnode dec_ref. Per-process идентичность (`cwd`, `umask`, `uid`, `gid`, `euid`, `egid`) хранится атомарно в структуре `Process`, наследуется при `fork()` и синхронизируется в `vfs.ctx` при каждом вызове syscall.

---

### Сетевой стек

<details>
<summary><b>Драйверы сетевых карт</b></summary>

| Драйвер | Чип |
|:--|:--|
| **Intel E1000** | 82540EM, 82545EM, 82574L, 82579LM, I217 |
| **Realtek RTL8139** | RTL8139 |
| **Realtek RTL8168** | RTL8168, RTL8169 |
| **VirtIO Net** | QEMU/KVM виртуальная сетевая карта |

</details>

<details>
<summary><b>Протоколы</b></summary>

| Уровень | Протоколы |
|:--|:--|
| **L2** | Ethernet, ARP (таблица кэша + валидация заголовков) |
| **L3** | IPv4, ICMP |
| **L4** | UDP, TCP (listener + client, state machine, ретрансмиты) |
| **Приложение** | DHCP, DNS, NTP, HTTP/1.1, HTTP/2 (HPACK), Ping, Traceroute |
| **Безопасность** | TLS 1.2 / 1.3 (ECDHE + RSA + AES-GCM, constant-time) |
| **Userspace сокеты** | AF_INET/SOCK_STREAM через syscall 56-59; `SOCK_FD_BASE=4096`; блокирующий TCP-клиент, таймаут 30 с; до 64 сокетов |

**netd** - сервис mikuD, зарегистрированный на таргете `MultiUser`, автоматически выполняет DHCP после появления линка, не блокируя загрузку.

</details>

<details>
<summary><b>TLS 1.2 / 1.3: полная реализация с нуля</b></summary>

- ECDH: обмен ключами P-256 ECDHE (`tls_ecdh.rs`), constant-time скалярное умножение (always-double-always-add + `cmov`)
- RSA: парсинг ASN.1/DER сертификатов, PKCS#1 v1.5 (`tls_rsa.rs`), байты паддинга из RDRAND
- BigNum: собственная реализация больших чисел для RSA 2048-bit (`tls_bignum.rs`)
- AES-GCM: аутентифицированное симметричное шифрование (`tls_gcm.rs`)
- SHA-256, HMAC, HKDF: хэширование, вывод ключей (`tls_crypto.rs`)
- Рукопожатие: ClientHello -> ServerHello -> Certificate -> [ECDHE] -> Finished (client и server Finished verify_data проверяются)
- HTTP/2: кадры по RFC 7540 и HPACK по RFC 7541 с корректной таблицей Хаффмана из Приложения B (`http2.rs`)

#### Усиление безопасности

| Угроза | Меры |
|:--|:--|
| **RNG** | CSPRNG на RDRAND для ClientHello random, CBC IV, приватного ключа ECDH, паддинга RSA (`random::random_u64`) |
| **Timing (Lucky13)** | Constant-time сравнение MAC через OR-аккумулятор |
| **Padding oracle** | Полная проверка паддинга по RFC 5246 - все байты, не только последний |
| **ECDH timing leak** | `fe_cmov` / `jac_cmov` XOR-маска для выбора поля/точки |
| **Подмена сервера** | TLS 1.2 `verify_data = PRF(master, "server finished", hs_hash)` проверяется constant-time |
| **PKCS#1 паддинг** | Ненулевые байты из RDRAND (rejection loop) |
| **ARP spoofing** | Проверка hw_type / proto_type / hlen / plen до приёма ARP-IPv4 записи |

</details>

---

### VFS (виртуальная файловая система)

<details>
<summary><b>Развернуть</b></summary>

#### Основные возможности

| Параметр | Значение |
|:--|:--|
| **Количество VNode** | 256 |
| **Одновременно открытых файлов** | 32 |
| **Точки монтирования** | 8 |
| **Дочерние узлы** | Динамически (без ограничений) |

Дочерние узлы управляются через динамическую `Vec`-based хэш-таблицу. Начальное количество слотов 16, при заполнении 75% автоматически удваивается.

- Типы узлов: `Regular`, `Directory`, `Symlink`, `CharDevice`, `BlockDevice`, `Pipe`, `Fifo`, `Socket`
- Полные метаданные: права, uid/gid, временные метки, размер, nlinks

#### Системные библиотеки

При загрузке создается директория `/lib` в tmpfs, `libmiku.so` записывается как immutable файл.
Флаг immutable запрещает unlink / write / rename.

#### Кэш

| Кэш | Размер |
|:--|:--|
| **Page cache** | 128 страниц x 512 байт, LRU вытеснение |
| **Dentry cache** | 128 записей, FNV32 хэш |

#### Навигация

- Path walking: глубина до 32 компонентов
- Разрешение символических ссылок: защита от циклов (8 уровней)
- FNV32 хэш: O(1) поиск по имени

#### Безопасность

- UNIX модель прав: `owner/group/other`, `setuid/setgid/sticky`
- Метки безопасности (MAC), квоты по байтам и inode
- Блокировки файлов: shared/exclusive с обнаружением deadlock (до 16 блокировок)
- Флаг immutable: защита системных библиотек

#### Продвинутые возможности

| Возможность | Детали |
|:--|:--|
| **VFS журнал** | 16 записей операций |
| **Xattr** | 8 расширенных атрибутов на узел |
| **Notify события** | inotify-подобная подсистема (до 16 событий) |
| **Хранилище версий** | 16 снапшотов файлов |
| **CAS хранилище** | Контентно-адресуемая дедупликация (до 16 объектов) |
| **Очередь блочного I/O** | 8 асинхронных запросов |

</details>

---

### Файловые системы

| FS | Точка монтирования | Описание |
|:--:|:--:|:--|
| **tmpfs** | `/` | RAM-based корневая FS |
| **devfs** | `/dev` | Устройства: `null`, `zero`, `random`, `urandom`, `console`, плюс сырые блочные узлы `blkN` / `blkNpM` (major 8) |
| **procfs** | `/proc` | `version`, `uptime`, `meminfo`, `mounts`, `cpuinfo`, `stat`, `heap`, `diskstats` |
| **ext2** | `/mnt` | Полная запись/чтение реального диска |
| **ext3** | `/mnt` | Журналирование (JBD2) поверх ext2, отложенная запись |
| **ext4** | `/mnt` | Файлы на основе экстентов + crc32c контрольные суммы |

---

### MikuFS: драйвер Ext2/3/4

<details>
<summary><b>Развернуть</b></summary>

#### Чтение

- Суперблок, дескрипторы групп, inode, записи директорий
- Непрямые блоки (одинарные / двойные / тройные)
- Дерево экстентов Ext4

#### Запись

- Создание и удаление файлов, директорий, символических ссылок
- Bitmap аллокатор для блоков и inode (с приоритетом групп)
- Рекурсивное удаление
- Отложенная запись (dirty cache + pdflush)

#### Ext3 журнал (JBD2)

- Создание журнала (конвертация `ext2 -> ext3`)
- Запись транзакций: descriptor block, commit block, revoke block
- Восстановление: воспроизведение незавершенных транзакций при монтировании
- Отложенный коммит: ускорение записи журнала через dirty cache

#### mkfs

- Форматирование ext2/ext3/ext4
- Lazy init: немедленная инициализация только метаданных group 0, остальное отложено
- Инициализация только суперблока журнала (без обнуления всех блоков)

#### Утилиты

- `fsck`, `tree`, `du`, `cp`, `mv`, `chmod`, `chown`, hard links

</details>

---

### Команды оболочки

#### Управление сервисами (sv)

| Команда | Описание |
|:--|:--|
| `sv list` | Список всех сервисов |
| `sv status <name>` | Детальный статус |
| `sv start/stop/restart <name>` | Жизненный цикл сервиса |
| `sv reload <name>` | Отправка SIGHUP |
| `sv enable/disable <name>` | Включение/выключение |
| `sv mask/unmask <name>` | Маскирование/размаскирование |
| `sv force-stop <name>` | Принудительное завершение |
| `sv journal [name]` | Лог событий |
| `sv target [name]` | Управление таргетами |
| `sv analyze` | Анализ загрузки |
| `sv tree/rdeps <name>` | Информация о зависимостях |
| `sv load/scan` | Управление unit-файлами |
| `sv timer list/start/stop` | Управление таймерами |

#### Унифицированные ext команды (автоопределение версии FS)

| Команда | Синтаксис | Описание |
|:--|:--|:--|
| `ext2mount` | `ext2mount [drive]` | Монтирование ext2 |
| `ext3mount` | `ext3mount [drive]` | Монтирование ext3 |
| `ext4mount` | `ext4mount [drive]` | Монтирование ext4 |
| `extls` | `extls [path]` | Список директории |
| `extcat` | `extcat <path>` | Содержимое файла |
| `extstat` | `extstat <path>` | Детали inode |
| `extinfo` | `extinfo` | Информация суперблока |
| `extwrite` | `extwrite <path> <text>` | Запись в файл |
| `extappend` | `extappend <path> <text>` | Дозапись в файл |
| `exttouch` | `exttouch <path>` | Создание пустого файла |
| `extmkdir` | `extmkdir <path>` | Создание директории |
| `extrm` | `extrm [-rf] <path>` | Удаление файла |
| `extrmdir` | `extrmdir <path>` | Удаление пустой директории |
| `extmv` | `extmv <path> <newname>` | Переименование файла |
| `extcp` | `extcp <src> <dst>` | Копирование файла |
| `extln -s` | `extln -s <target> <link>` | Создание символической ссылки |
| `extlink` | `extlink <existing> <link>` | Создание жесткой ссылки |
| `extchmod` | `extchmod <mode> <path>` | Изменение прав |
| `extchown` | `extchown <uid> <gid> <path>` | Изменение владельца |
| `extdu` | `extdu [path]` | Использование диска |
| `exttree` | `exttree [path]` | Дерево директорий |
| `extfsck` | `extfsck` | Проверка целостности FS |
| `extcache` | `extcache` | Статистика блочного кэша |
| `extcacheflush` | `extcacheflush` | Сброс кэша |
| `extsync` / `sync` | `sync` | Запись на диск |

> Старые команды (`ext2ls`, `ext3cat`, `ext4write` и т.д.) оставлены для обратной совместимости.

#### VFS команды

| Команда | Описание |
|:--|:--|
| `ls [path]` | Список директории (ext + VFS объединенный вид) |
| `cd <path>` | Смена директории |
| `pwd` | Текущий путь |
| `mkdir <path>` | Создание директории |
| `touch <path>` | Создание файла (RAM) |
| `cat <path>` | Содержимое файла |
| `write <path> <text>` | Запись в файл (RAM) |
| `dd if= of= [bs= count= skip= seek= conv=notrunc,fsync]` | Копирование блоков между файлами, `/dev/zero` и сырыми узлами `/dev/blkN` |
| `rm [-rf] <path>` | Удаление файла/директории |
| `rmdir <path>` | Удаление директории (ext совместимо) |
| `mv <old> <new>` | Переименование |
| `stat <path>` | Информация о файле |
| `chmod <mode> <path>` | Изменение прав |
| `df` | Информация о файловой системе |

#### Команды динамической линковки

| Команда | Описание |
|:--|:--|
| `exec <path>` | Запуск ELF бинаря (с динамической линковкой) |
| `ldconfig` | Обновление кэша разделяемых библиотек |
| `ldd` | Список кэшированных библиотек |

#### Управление процессами

| Команда | Описание |
|:--|:--|
| `ps` | Список всех процессов |
| `top` | Онлайн-монитор процессов |
| `kill <pid>` | Завершение процесса по PID |
| `nice <pid> <prio>` | Изменение приоритета (1-20) |
| `affinity <pid> <mask>` | Установка CPU-аффинности |
| `swaptest` | Стресс-тест подсистемы swap |

#### Команды NVIDIA GPU

| Команда | Описание |
|:--|:--|
| `nvidia` / `nvidia info` | Сводка GPU: PCI, чип, BAR0/1/3, PTIMER, MSI, scanout |
| `nvidia debug` | Полный дамп регистров BAR0 (PMC, PBUS, PFIFO, PTOP, PTIMER) |
| `nvidia firmware` | Список встроенных blob-файлов TU116 с заголовками NVFW |
| `nvidia falcon` | Живучесть движков: SEC2, GSP, NVDEC, FECS, GPCCS0/1 |
| `nvidia ungate` | Установка PMC_ENABLE.GR + CE0 (активация FECS / GPCCS / CE0) |
| `nvidia pmc-scan` | Только чтение: обход области PMC (0x000..0x1000) |
| `nvidia dma-state` | Снимок регистров DMATRF + статус IDLE/ERROR по движкам |
| `nvidia fbif-scan` | Обход окна FBIF (+0x500..+0xa00) для каждого активного движка |
| `nvidia fbif-decode` | Декодирование всех 8 слотов TRANSCFG для каждого активного движка |
| `nvidia dma-test` | Сквозной DMA loopback: sysmem -> SEC2 DMEM (256 байт, паттерн CAFE) |
| `nvidia imem-test` | IMEM-вариант DMA loopback: sysmem -> SEC2 IMEM |
| `nvidia acr-info` | Структурный дамп каждого ACR blob SEC2 (контейнер NVFW + заголовки HS) |
| `nvidia gsp` | First-contact загрузка GSP через gsp::attempt_boot |
| `nvidia gsp-rm` / `gsprm` | Подготовка staging GSP-RM (VRAM probe, layout WPR2, sysmem alloc) |
| `nvidia gsp-rm-dryrun` | Сборка radix3 таблицы и проверка целостности цепочки |
| `nvidia gsp-rm-load` | Загрузка подписанного GSP-RM blob в WPR2 (MissingFirmware если отсутствует) |
| `nvidia gsp-rm-boot` | Запуск GSP booter HS-образа и наблюдение результата |
| `nvidia sec2-acr` / `sec2-acr-v2` | SEC2 ACR first-contact (загрузка ahesasc + запуск bl) |
| `nvidia wpr-state` | Дамп состояния регистров WPR / WPR2 |
| `nvidia msgq` | Self-test колец CMDQ/MSGQ (только host-side framing) |
| `nvidia rpc` | Self-test framing RPC-заголовков GSP-RM |
| `nvidia temp` | PTHERM температура кристалла + пороги slowdown/shutdown |
| `nvidia next` | Анализ состояния и рекомендация следующего шага разработки драйвера |
| `nvidia splash` | Перерисовка загрузочного экрана через фреймбуфер |

#### Системные команды

| Команда | Описание |
|:--|:--|
| `poweroff` / `shutdown` / `halt` | Graceful shutdown через mikuD |
| `reboot` / `restart` | Graceful reboot через mikuD |
| `info` | Информация о системе |
| `memmap` | Карта физической памяти |
| `heap` | Статистика кучи |
| `clear` | Очистка экрана |
| `echo <text>` | Вывод текста |
| `history` | История команд |
| `help` | Список команд |

#### mkfs / диски / swap

| Команда | Описание |
|:--|:--|
| `mkfs.ext2 <drive>` | Форматирование ext2 |
| `mkfs.ext3 <drive>` | Форматирование ext3 (с журналом) |
| `mkfs.ext4 <drive>` | Форматирование ext4 (экстенты + журнал) |
| `blkstat` | Показать все блочные устройства (ATA/AHCI/NVMe/virtio-blk) + дерево GPT-разделов + BIO-очередь + статистика кэша |
| `blkdiscard <drive> [lba count]` | Discard/TRIM диапазона секторов (без диапазона - весь диск); аналог blkdiscard(8) |
| `blkzero <drive> <lba> <count>` | Обнуление диапазона секторов (NVMe/virtio Write Zeroes, фоллбэк - запись нулей) |
| `fstrim` | Discard всех свободных блоков активной смонтированной ext-ФС по битмапам групп; аналог fstrim(8) |
| `blkonline <drive>` | Вернуть устройство, выведенное failfast в offline, обратно online (сбрасывает серию ошибок) |
| `smart <drive>` | SMART / NVMe отчёт здоровья: статус, температура, износ, часы работы, объём R/W |
| `mkfs.dry <drive> <ext2\|ext3\|ext4>` | Dry-run форматирование (только layout) |
| `gpt <drive>` | Показать таблицу GPT |
| `gpt.init <drive>` | Инициализировать пустой GPT |
| `gpt.add <drive> <spec>` | Добавить раздел |
| `gpt.del <drive> <partition>` | Удалить раздел |
| `partprobe [drive]` | Перечитать GPT и обновить узлы `/dev/blkNpM` в рантайме (partprobe(8)) |
| `mkswap <drive> <partition>` | Создать swap на разделе |
| `swapon <drive> <partition>` | Активировать swap |
| `swapon.raw <drive> <start> <size>` | Активировать swap по сырым координатам |
| `swapon.auto` | Автоподбор и активация swap-разделов |
| `swapoff` | Деактивировать swap |
| `swapinfo` | Использование swap |
| `mkswap.raw <drive> <start> <size>` | Создать сырой swap без GPT |

#### Расширенные атрибуты / флаги

| Команда | Описание |
|:--|:--|
| `getxattr <path> <name>` | Прочитать user xattr |
| `setxattr <path> <name> <value>` | Записать user xattr |
| `listxattr <path>` | Список всех xattr |
| `chattr <+/-flags> <path>` | Установить флаги файла (i=immutable, a=append, d=nodump, A=noatime) |
| `lsattr <path>` | Показать флаги файла |
| `fiemap <path>` | Карта экстентов файла (ext4) |

#### Сеть

| Команда | Описание |
|:--|:--|
| `net <subcmd>` | Сетевой статус / конфигурация |
| `dhcp` | Получить аренду по DHCP |
| `ping <ip\|host> [count]` | ICMP echo (резолв через DNS) |
| `ntp [server]` | Синхронизация времени через NTP |
| `traceroute` / `tr <host>` | Трассировка маршрута (UDP/ICMP) |
| `fetch <url\|host> [port]` | Минимальный HTTP/HTTPS-клиент |
| `wget <url> [-O <file>]` | Скачивание по HTTP(S) |
| `curl <url> [-X GET\|POST] [-d <data>] [-o <file>] [-I]` | HTTP(S)-клиент |

---

### Драйвер NVIDIA GPU

<details>
<summary><b>Развернуть</b></summary>

#### Обзор

MikuOS включает собственный драйвер для GPU NVIDIA эпохи GSP. Написан с нуля
на Rust без std, использует MMIO поверх HHDM.

> Turing - первое поколение NVIDIA с GSP (GPU System Processor) на встроенном ядре RISC-V.
> Без подписанного firmware GSP большинство движков недоступно.
> GTX 1650 (TU116/TU117) проходит полный путь: host-side probe + управление Falcon-движками + DMA loopback + подготовка GSP-RM.
> Любая другая карта NVIDIA (прочие Turing, Ampere, Ada, ...) распознаётся и поднимается host-side через generic-путь.

#### Поддерживаемые GPU

**Полный драйвер (встроенный firmware, конвейер GSP-RM):**

| Чип | SKU | Диапазон Device ID |
|:--|:--|:--|
| **TU117** | GTX 1650 GDDR5 / GDDR6, Mobile/Max-Q | 0x1F82..0x1FBA |
| **TU116** | GTX 1650 SUPER, GTX 1660 / 1660 Ti / 1660 SUPER | 0x2182..0x21C4 |

**Generic host-side bring-up (распознавание + диагностика, без firmware):**

Любой GPU NVIDIA, чья архитектура определяется по PMC_BOOT_0 - вся линейка
Turing / Ampere / Ada Lovelace (и новые семейства, читаются только на чтение
с Turing-картой регистров). Карта маппится, идентифицируется, проверяется
MSI/VBIOS и живучесть Falcon, затем регистрируется в общей таблице GPU
(`nvidia list`). Конвейер GSP-RM остаётся за per-chip firmware-бандлом,
который пока есть только у TU116.

#### Структура модулей (nvidia/)

| Модуль | Описание |
|:--|:--|
| **mod.rs** | Корень: точка входа probe, диспетчеризация (gtx1650 vs generic), глобальный ACTIVE_GTX1650 |
| **pci.rs** | PCI-сканирование (класс 0x03 + vendor 0x10DE), определение размера BAR |
| **mmio.rs** | MMIO-примитивы: volatile чтение/запись через HHDM |
| **chip.rs** | Идентификация чипа по PMC_BOOT_0; codename для Turing/Ampere/Hopper/Ada |
| **profile.rs** | Профиль чипа: базы Falcon-движков + наличие firmware-бандла |
| **generic.rs** | Host-side bring-up для любого GPU NVIDIA + реестр generic-GPU |
| **msi.rs** | Обход возможностей PCI MSI / MSI-X |
| **vbios.rs** | Извлечение образа VBIOS из PCI expansion ROM |
| **fb.rs** | Фреймбуфер: определение boot scanout, BAR-индекс и смещение |
| **gtx1650/** | Полный драйвер GTX 1650 / 1660 (TU117 + TU116), единственный чип со встроенным firmware |

#### Архитектуры чипов

| Код arch | Семейство | Примеры | Уровень драйвера |
|:--:|:--|:--|:--|
| 0x16 | Turing | TU102, TU104, TU106, TU116 (0x8), TU117 (0x7) | TU116/TU117 полный; прочие host-side |
| 0x17 | Ampere | GA100, GA102, GA103, GA104, GA106, GA107 | host-side |
| 0x18 | Hopper | GH100 | host-side |
| 0x19 | Ada Lovelace | AD102, AD103, AD104, AD106, AD107 | host-side |
| 0x1A/0x1B | Blackwell | GB10x / GB100 | host-side (только чтение) |

#### Falcon-движки

| Движок | Базовый адрес | Описание |
|:--|:--|:--|
| **SEC2** | PSEC_BASE | Движок безопасности: загрузка ACR, загрузка HS ucode |
| **GSP** | PGSP_BASE | GPU System Processor (RISC-V) |
| **NVDEC** | PNVDEC_BASE | Видеодекодер |
| **FECS** | PFECS_BASE | Переключение контекста фронтального движка |
| **GPCCS0/1** | PGPCCS_BASE | Переключение контекста GPC |

Состояния живучести: Alive, GatedPriSentinel, NoResponse, BadHwcfg.

#### DMA-путь (loopback через SEC2)

```
1. DmaBuffer::alloc(pages) - физически непрерывные страницы из PMM
2. Заполнение паттерном (0xCAFE_xxxx) + write_barrier (sfence)
3. Программирование SEC2 TRANSCFG[7]: NoncoherentSysmem + Physical addressing
4. Установка FBIF_CTL.ALLOW_PHYS_NO_CTX
5. Engine::dma_load: sysmem -> SEC2 DMEM/IMEM (256 байт, ctxdma=7)
6. PIO readback через FALCON_DMEM_C0/D0 (или IMEM_C0/D0)
7. Проверка паттерна + восстановление TRANSCFG
```

#### Пакет firmware (TU116)

| Blob | Движок | Контейнер |
|:--|:--|:--|
| acr/bl.bin | SEC2 | NVFW v1 |
| acr/ucode_ahesasc.bin | SEC2 | NVFW v1 |
| gsp/booter_load.bin | GSP | NVFW v1 |
| gsp/booter_unload.bin | GSP | NVFW v1 |
| nvdec/scrubber.bin | NVDEC | NVFW v1 |
| fecs/ucode.bin | FECS | raw |
| gpccs/ucode.bin | GPCCS | raw |

Все blob-файлы встроены в ядро через include_bytes! при компиляции.
Образ GSP-RM (gsp_t.bin) НЕ включен - требует NVIDIA open-kernel-modules.

#### Дорожная карта драйвера

| Шаг | Статус | Описание |
|:--:|:--:|:--|
| 1 | готово | PCI bind + BAR0 mapped |
| 2 | готово | Идентификация чипа (PMC_BOOT_0) |
| 3 | готово | Пакет firmware встроен |
| 4 | готово | Alive-проба Falcon-движков SEC2 / GSP |
| 5 | готово | FBIF scan + декодирование TRANSCFG |
| 6 | готово | DMA loopback (DMEM + IMEM) |
| 7 | - | Загрузка ACR через SEC2 (установка WPR) |
| 8 | wip | Первый контакт скруббера NVDEC (`nvdec::attempt_scrub`); полная подготовка дескриптора скраба ожидается |
| 9 | wip | Подготовка GSP-RM (`gsprm`) + полный orchestrator загрузки (`gsprm::boot`, `nvidia gsp-rm-boot-full`): scrub->load->ACR->WPR2->booter->MSGQ handshake. Blob GSP-RM встроен. Осталось 2 гейта: lock WPR2 в ACR (нужен `RM_FLCN_ACR_DESC` в SEC2 DMEM) и передача адреса очереди через GSP boot-args |
| 10 | - | Контексты FECS/GPCCS, доступ к PGRAPH |

</details>

---

### Блочный уровень и драйверы хранилищ

<details>
<summary><b>Блочный уровень (block layer)</b></summary>

#### Обзор

Блочный уровень - единая точка маршрутизации между файловыми системами и драйверами хранилищ, по образцу Linux generic block layer. Конкретные драйверы регистрируются один раз за стабильным `BlockDevId`; уровни выше никогда не держат драйвер напрямую.

| Параметр | Значение |
|:--|:--|
| **Device IDs** | 0-3: слоты legacy ATA; 4-7: PCI-устройства (AHCI, NVMe, virtio-blk) |
| **Макс. устройств** | 8 |
| **Учёт I/O** | BIO-очередь: счётчики submitted / completed / errors |
| **Блокировки** | Per-device mutex слота; ATA-слоты делят bus lock; PCI-устройства полностью параллельны |
| **Ретраи** | Transient-ошибки (timeout/fault) - до 2 прозрачных повторов; счётчики errors/retries на устройство |
| **Failfast-состояние** | Online/Degraded/Offline на устройство (по образцу SCSI); после 8 подряд пост-retry ошибок устройство уходит Offline и сразу возвращает ошибку вместо зависания на таймаутах. `blkonline <drive>` сбрасывает; состояние видно в `blkstat` и `/proc/diskstats` |
| **FUA-барьеры** | `block::write_barrier` коммитит с Force Unit Access (бит FUA у NVMe, ATA WRITE DMA FUA EXT) для commit-блока журнала ext3/4; бэкенды без FUA откатываются на запись+flush |
| **partprobe** | Узлы разделов `/dev/blkNpM` перечитываются из GPT в рантайме (`partprobe`), без перезагрузки |
| **Латентность** | Замер каждого запроса по TSC; средняя латентность в `blkstat` (аналог iostat await) |

#### API

| Функция | Описание |
|:--|:--|
| `block::probe()` | Обход PCI-шины: регистрирует AHCI-порты, virtio-blk и NVMe в IDs 4-7 |
| `block::read(dev, lba, count, buf)` | Кэшированное чтение; последовательные промахи запускают readahead |
| `block::write(dev, lba, count, buf)` | Write-back: данные попадают в кэш, запись на диск - при flush/вытеснении |
| `block::write_sync(dev, lba, count, buf)` | Write-through: запись завершается до возврата (журналы, GPT, swap) |
| `block::flush(dev)` | Сброс грязных чанков (elevator-порядок) + flush volatile-кэша устройства |
| `block::discard(dev, lba, count)` | Discard/TRIM диапазона секторов; полностью покрытые чанки кэша сбрасываются (включая грязные) до команды устройству |
| `block::write_zeroes(dev, lba, count)` | Обнуление диапазона; нативный Write Zeroes (NVMe/virtio) для выровненной середины, обычная запись по краям, фоллбэк - запись нулей |
| `MikuFS::trim_free_blocks(minlen)` | FITRIM: обход битмапов групп смонтированной ФС, discard серий свободных блоков (команда `fstrim`); mkfs.* предварительно discard-ит всю область |
| `block::info(dev)` | Геометрия / идентичность устройства (включая флаг `discard`) |
| `block::cache_stats()` | `(hits, misses, readaheads, write_merges, dirty)` |
| `block::io_stats()` | `(submitted, completed, errors)` из BIO-очереди |
| `block::dev_stats(dev)` | `(kind, sectors_read, sectors_written, sectors_discarded, ios, avg_io_us)` на устройство |
| `block::health(dev)` | SMART / NVMe снимок здоровья; `None` если бэкенд не поддерживает |

#### Буферный кэш

| Параметр | Значение |
|:--|:--|
| **Гранулярность** | 4 KiB чанки (8 секторов на чанк) |
| **Объём** | 512 чанков × 4 KiB = **2 MiB** |
| **Организация** | 8-way set-associative, 64 набора, per-set LRU |
| **Политика** | Write-back; `write_sync` - write-through для упорядоченных записей |
| **Readahead** | Адаптивный: 32 KiB на свежий последовательный поток, до 64 KiB (16 чанков) на устойчивый |
| **Грязный лимит** | Flush при 256 грязных чанках (high-water mark) |
| **Слияние записей** | Смежные грязные чанки объединяются в одну команду драйвера до 64 KiB при writeback |
| **bdflush** | Фоновый сервис mikuD: каждые 2 с сбрасывает грязные чанки на диск (элеваторная развёртка по LBA) |
| **Когерентность** | Все дисковые обращения ядра идут через `crate::block`; второго пути нет |

</details>

<details>
<summary><b>Драйверы хранилищ</b></summary>

#### AHCI (SATA)

| Параметр | Значение |
|:--|:--|
| **PCI-класс** | 01.06 (Mass Storage / SATA AHCI) |
| **Регистры** | BAR5 (ABAR) MMIO, mapped uncached через HHDM |
| **Макс. портов** | 4 SATA-диска за probe |
| **Команды** | READ DMA EXT, WRITE DMA EXT, FLUSH CACHE EXT, IDENTIFY, DATA SET MANAGEMENT (TRIM) |
| **Завершение** | Полинг PxCI |
| **Буфер** | 64 KiB bounce buffer, одна PRD-запись |
| **TRIM** | Поддержка по слову 169 IDENTIFY; 8-байтные диапазоны, 64 на 512-байтный блок |

#### NVMe

| Параметр | Значение |
|:--|:--|
| **Очереди** | 1 admin queue pair (глубина 16) + **4 I/O queue pairs (глубина 64)**, маршрутизация по CPU, per-queue блокировки - несколько CPU параллельно submit/poll (blk-mq) |
| **Передача** | До 128 секторов (64 KiB) на команду через PRP1 + PRP list page |
| **Завершение** | Полинг CQ phase bit |
| **Память** | Один page-aligned аллок: admin SQ/CQ, I/O SQ/CQ, PRP list, IDENTIFY, bounce |
| **Опкоды** | NVM READ (0x02), NVM WRITE (0x01), NVM FLUSH (0x00), NVM DSM (0x09, deallocate = discard) |
| **Discard** | Dataset Management с атрибутом deallocate; поддержка по биту 2 ONCS |
| **Write Zeroes** | Команда Write Zeroes (опкод 0x08); поддержка по биту 3 ONCS |
| **Здоровье** | Get Log Page (LID 0x02) - SMART/Health Information (512 байт): температура, износ, POH, объём R/W |

#### virtio-blk (legacy/transitional)

| Параметр | Значение |
|:--|:--|
| **Транспорт** | Legacy virtio-pci, port I/O (BAR0) |
| **Кольцо** | Layout вычисляется runtime из размера очереди устройства |
| **Макс. очередь** | 256 дескрипторов |
| **Передача** | До 128 секторов (64 KiB) на запрос; большие - чанкуются block layer |
| **Возможности** | FLUSH (bit 9), DISCARD (bit 13) и WRITE_ZEROES (bit 14) согласованы; discard/обнуление ограничены `max_discard_sectors` / `max_write_zeroes_sectors` из конфига |

#### ATA (legacy PIO)

| Параметр | Значение |
|:--|:--|
| **Режим** | PIO (программный I/O) |
| **Операции** | Чтение/запись секторов (512 байт), до 255 секторов/команда |
| **Количество дисков** | 4: Primary/Secondary × Master/Slave (IDs 0-3) |
| **Защита** | Flush кэша после записи, таймаут 50K итераций |
| **Адресация** | LBA28 (до 128 ГБ) + **LBA48** (READ/WRITE EXT, 48-бит адресация) |
| **DMA** | Определение и отслеживание возможности bus-master DMA |
| **TRIM** | DATA SET MANAGEMENT через bus-master DMA; поддержка по слову 169 IDENTIFY |
| **Здоровье** | SMART RETURN STATUS (cmd 0xB0/feature 0xDA): подпись LBA mid/high - здоровый или отказывающий |

</details>

---

## Сборка и запуск

### Необходимые инструменты

| Инструмент | Назначение |
|:--|:--|
| **Rust nightly** | `no_std` + нестабильные возможности компилятора |
| **QEMU** | Эмуляция x86_64 машины |
| **grub-mkrescue** | Создание загрузочного ISO |
| **GCC** | Генерация stub libmiku + компиляция C программ |
| **e2tools** | Копирование файлов на ext4 образ |
| **Cargo** | Сборка ядра |

### Порядок запуска

```bash
git clone https://github.com/alunwrd/miku-os
cd miku-os/builder
cargo run
```

Builder делает все автоматически:

```
Режим экономии RAM? (y/N)
[1/7] Компиляция ld-miku.so
[2/7] Компиляция libmiku.so
[3/7] Компиляция ядра miku-os
[4/7] Создание файловой структуры
[5/7] Генерация системного образа (miku-os.iso)
[6/7] Подготовка диска
[7/7] Запуск QEMU (опционально (y/N))
```

### Сборка userspace программ

```bash
cd src/lib/userspace
./build.sh hello         # сборка + копирование на диск
./build.sh test_full     # тестовый набор
./build.sh               # все бинари
```

---

## MikuOS ABI

Полная документация по разработке userspace программ: [MikuOS_ABI.md](docs/MikuOS_ABI.md)

---

## Автор

<div align="center">
  <a href="https://github.com/alunwrd">
    <img src="https://github.com/alunwrd.png" width="100" style="border-radius:50%;" alt="alunwrd">
  </a>
  <br><br>
  <a href="https://github.com/alunwrd"><b>@alunwrd</b></a>
  <br>
  <sub>Автор и единственный разработчик Miku OS</sub>
  <br>
  <sub>Ядро - VFS - MikuFS - ELF - ld-miku - libmiku - Оболочка - Сеть - TLS - Планировщик - PMM - VMM - Swap - mikuD - Сигналы - fork/exec - ACPI - APIC - SMP - Драйвер NVIDIA GPU - Блочный уровень - AHCI/NVMe/virtio-blk</sub>
</div>

---

## От автора

Удачного использования :)

<div align="center">

**Miku OS** - чистая ОС, написанная с нуля на Rust

*С любовью*

<img src="https://raw.githubusercontent.com/alunwrd/miku-os/main/docs/miku.png" width="220" alt="Miku Logo">
