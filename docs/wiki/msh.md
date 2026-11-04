# msh, the shell

`msh` (`/bin/msh`, `system/src/msh/`) is MikuOS's command shell: a ring-3
program on a pseudo-terminal, started in every terminal window by `mayterm`.
It runs programs from `/bin`, forwards names it cannot find to the kernel
console through `kcmd`, and has quoting, pipelines, redirections, `;`, `&&`,
`||`, background jobs with job control, a line editor with history, and
nothing more: it has no variables, no expansion of any kind, no globbing and
no control flow. This page is the whole language, its builtins, how commands
are found, the editor's keys and where it differs from POSIX `sh`.

## Contents

- [Starting msh](#starting-msh)
- [The prompt](#the-prompt)
- [The language](#the-language)
  - [Words and quoting](#words-and-quoting)
  - [Comments](#comments)
  - [Commands, pipelines and lists](#commands-pipelines-and-lists)
  - [Redirections](#redirections)
  - [Environment assignments](#environment-assignments)
  - [What is not in the language](#what-is-not-in-the-language)
- [Builtins](#builtins)
- [How commands are found](#how-commands-are-found)
- [Jobs and job control](#jobs-and-job-control)
- [The line editor and history](#the-line-editor-and-history)
- [Exit statuses](#exit-statuses)
- [Limits](#limits)
- [Differences from POSIX sh](#differences-from-posix-sh)
- [Examples](#examples)
- [Where the code is](#where-the-code-is)

---

## Starting msh

| Invocation | What it does |
|---|---|
| `msh` | interactive: a banner, then prompts until `exit` or end of input |
| `msh -c 'line'` | run one line and exit with its status; no banner, no prompt |
| `msh file` | run every line of the file, in order, and exit with the last status; `msh: file: cannot read` and 127 if it cannot be read |
| `msh -l ...`, `-i`, `-li`, `-il`, `--login` | accepted before `-c` or a file and ignored (Zed runs `$SHELL -l -i -c '...'` to learn a login's environment) |

**There are no startup files.** No `/etc/msh.rc`, no `~/.mshrc`, no
profile is read, interactive or not: everything a shell starts with comes from
its environment.

Who starts it: `mayterm` forks, attaches the child to its pty
(`tty(TTY_ATTACH)`) and `execve`s `/bin/msh` with the terminal's
environment. A login session's environment (`PATH=/bin`, `HOME`, `USER`,
`LOGNAME`, `SHELL=/bin/msh`, `TERM`) comes from `mikudod`
([system.md](system.md)). Running `msh` from inside `msh` nests a shell on the
same terminal.

On start the banner is printed first, before anything that could fail, so a
window that shows it has a running shell:

```
MikuOS - tty 0x302, job 0x2a
welcome, miku - 'help' for what this shell knows :)
```

The terminal and job numbers are printed in hexadecimal.

## The prompt

```
miku@os:/home/miku $ 
```

The user name (from `/etc/passwd` for the real uid, or the number), `@os`,
`:`, the working directory, then ` $ ` (` # ` for root). The name is in blue
for an ordinary user and **red for root**. It is coloured with ANSI escapes and
cannot be configured: there is no `PS1`.

## The language

```text
line     := list ( (';' | '&') list )* [';' | '&']
list     := pipeline ( ('&&' | '||') pipeline )*
pipeline := command ( '|' command )*
command  := word+ ( '<' w | '>' w | '>>' w | '2>' w | '2>>' w | [n]'>&'m )*
```

One line is one unit: a script is run line by line, and nothing continues onto
the next line.

### Words and quoting

Words are separated by spaces and tabs. Quoting is resolved into the words
themselves before anything else happens:

| Form | Meaning |
|---|---|
| `'text'` | literal, every character |
| `"text"` | literal, except that `\"`, `\\`, `\$` and `` \` `` drop the backslash; any other backslash stays |
| `\c` | the character `c`, literally (`a\ b` is one word) |
| `ab'cd'"ef"` | quoted and unquoted parts join into one word |

An unclosed quote or a trailing backslash is an error (`msh: a ' with no
closing '`, status 2). An operator character inside a word ends the word, so
`a>b` is `a`, `>`, `b`; quote it to keep it.

### Comments

A `#` where a word would start begins a comment: the rest of the line is
ignored. A `#` inside a word (`a#b`) is part of the word.

### Commands, pipelines and lists

| Syntax | Meaning |
|---|---|
| `cmd args` | run a command and wait for it |
| `a \| b \| c` | a pipeline: each command's standard output to the next one's input, all in one process group (one job); the status is the **last** command's |
| `a ; b` | one after the other |
| `a && b` | `b` only if `a` succeeded (status 0) |
| `a \|\| b` | `b` only if `a` failed |
| `a &` | run in the background, keep the terminal for the shell; prints `[job] pgid` |
| `a && b &` | the whole chain in the background, in a forked shell of its own |

A skipped pipeline leaves the status where it was: `false && echo x` reports
`false`'s status. `&&` and `||` have equal precedence and bind left to right,
as in `sh`.

### Redirections

| Syntax | Effect |
|---|---|
| `< file` | standard input from `file` |
| `> file` | standard output to `file`, created with mode 0644 (less the umask) or truncated |
| `>> file` | standard output appended to `file` |
| `2> file`, `2>> file` | standard error to `file`, truncating or appending |
| `n>&m` | descriptor `n` becomes a copy of `m`; `n` and `m` are single digits |
| `>&m` | `1>&m` |

`2>` and `n>&m` are only recognised where a word would start (`echo a2>b`
writes `a2` to `b`). A file that cannot be opened prints `msh: file: cannot
open` and the command does not run (status 1).

**Order**: the file redirections (`<`, then `>`/`>>`, then `2>`/`2>>`) are
applied first and the `n>&m` duplications after them, **whatever order they
were written in**. So `cmd > f 2>&1` and `cmd 2>&1 > f` both send both streams
to `f` (in POSIX `sh` the second sends stderr to the old stdout). At most one
of each file redirection per command; a second replaces the first.

### Environment assignments

```sh
NAME=value cmd args
```

Leading words of the form `NAME=value` (a letter or `_`, then letters, digits
and `_`, then `=`) are put into the environment of that command only. A line
of nothing but assignments is an error (`msh: NAME=value sets a program's
environment, and no program follows`): there are no shell variables to set.
A command with assignments is never sent to the kernel console.

### What is not in the language

msh deliberately has none of these:

- variables and parameter expansion (`$VAR`, `${...}`, `$?`, `$0`); a `$` is
  an ordinary character;
- command substitution (`` `...` ``, `$(...)`), arithmetic (`$((...))`);
- globbing (`*`, `?`, `[...]` are passed literally), brace expansion;
- **tilde expansion** (`~` is an ordinary character: write `/home/miku`);
- here-documents (`<<`) and here-strings;
- `if`, `for`, `while`, `case`, functions, subshells `( )` and groups `{ }`;
- aliases, `export`, `set`, `source`/`.`, `alias`, `history`, `type`, `read`;
- multi-line constructs and line continuation.

The design note in `system/src/msh/main.rs` says why: msh is for running
programs, not for being programmed in. For logic, write a program or run a
script with a richer interpreter.

## Builtins

| Builtin | Synopsis | What it does |
|---|---|---|
| `cd` | `cd [dir]` | change the working directory; with no argument, `/` (not `$HOME`); `cd: dir: no` and status 1 if it fails |
| `exit`, `quit` | `exit [code]` | end the shell with `code` (0 if missing or not a number) |
| `jobs` | `jobs` | list jobs: `[n]+` marks the current one, then running or stopped and the process group |
| `fg` | `fg [n]` | bring job `n` (default: the current job) to the foreground and continue it (`SIGCONT`); wait for it |
| `bg` | `bg [n]` | continue a stopped job in the background |
| `wait` | `wait` | wait for every background job to finish |
| `echo` | `echo [word...]` | print the words separated by spaces and a newline; no options (`-n` is printed); `/bin/echo` has `-n` |
| `pwd` | `pwd` | print the working directory |
| `tty` | `tty` | `tty <id>  job <pgid>  foreground <pgid>` in hexadecimal |
| `help` | `help` | a summary of all of this |

`cd`, `exit`, `jobs`, `fg`, `bg` and `wait` act on the shell itself and take
effect only as a plain foreground command (no pipe, redirection or `&`).
`echo`, `pwd`, `tty` and `help` run in the forked child, so they work in
pipelines and with redirections. Every other name is a program.

## How commands are found

1. A builtin, as above.
2. If the name contains `/`, that path.
3. Otherwise **`/bin/<name>`**. There is no `PATH` search: `/bin` is the only
   directory, whatever `PATH` says (it is still passed on to programs).
4. If `execve` fails, the name has no `/` and the command has no assignments,
   the whole command line is sent to the kernel console with `kcmd` (104). The
   kernel answers for its own commands (`nvidia`, `mount`, `sv`, `dmesg`, the
   selftests, ...): read-only ones run, privileged ones print a refusal telling
   you to use `mikudo`. Their output goes to the terminal, not to the
   command's standard output, so it cannot be redirected or piped.
5. Otherwise `msh: <name>: not found in /bin`, status 127. For `sudo` and
   `doas` it adds a pointer to `mikudo`.

Every program gets the shell's environment plus the command's assignments.
The kernel console's command list is in [commands.md](commands.md#kernel-console).

## Jobs and job control

Every command or pipeline runs in a **process group** of its own (`setpgid` on
both sides of the `fork`, so there is no window in which a key reaches the
shell instead). For a foreground job the terminal's keys are handed to the
group (`tty(TTY_SETFG)`, which is `tcsetpgrp`) and taken back when it ends, so
Ctrl-C (`SIGINT`), Ctrl-\\ (`SIGQUIT`) and Ctrl-Z (`SIGTSTP`) reach the job and
not the shell.

- **Ctrl-Z** stops the foreground job: the shell waits with `WUNTRACED`, takes
  the terminal back and prints `[n] stopped  <pgid>`; `fg` and `bg` continue
  it.
- **`&`** starts a job in the background; the shell keeps the terminal. A
  background job that reads the terminal is stopped by `SIGTTIN` (or reads end
  of file if it ignores that).
- Finished background jobs are reaped and announced (`[n] done  <pid>`) just
  before the next prompt.
- At most 8 jobs are tracked at once.
- When the terminal's keys belong to another job, a read returns end of file,
  and the shell says so and exits: `msh: this terminal's keys belong to job
  ..., not to mine (...) - nothing to read, ending`.

The kernel side of process groups and terminals is in
[processes.md](processes.md#job-control-signals) and
[input-and-terminals.md](input-and-terminals.md).

## The line editor and history

When standard input is a terminal, msh edits the line itself
(`system/src/msh/edit.rs`): it puts the terminal in raw mode **only while its
prompt is up** and restores it before any program runs, so no program inherits
it. Where a key exists in the kernel's line discipline too, it means the same
thing here. With no terminal (a pipe, a file) there is no editor, only the
plain read.

| Key | Action |
|---|---|
| Left / Ctrl-B, Right / Ctrl-F | move one character |
| Home / Ctrl-A, End / Ctrl-E | start / end of the line |
| Up / Ctrl-P, Down / Ctrl-N | previous / next history entry (the line being typed is kept and comes back) |
| Backspace (DEL or Ctrl-H) | delete before the cursor |
| Delete | delete under the cursor |
| Ctrl-D | on an empty line: end of input (the shell exits); otherwise delete under the cursor |
| Ctrl-K | delete to the end of the line |
| Ctrl-U | delete the **whole** line (the line discipline's meaning, not bash's kill-to-start) |
| Ctrl-W | delete trailing spaces, then the word before them |
| Ctrl-L | clear the screen |
| Ctrl-C | abandon the line (`^C`), new prompt |
| Enter | run the line |

UTF-8 input is handled as characters. The cursor is placed with `CSI n G` and
the rest of the line cleared with `CSI K`, so the editor needs a terminal that
understands cursor addressing (MikuOS's terminals do).

**History** holds the last 32 lines, in memory only (it is gone when the shell
exits; there is no history file and no `history` builtin). A line longer than
256 bytes is not stored at all rather than stored cut short.

## Exit statuses

| Status | When |
|---|---|
| the command's | a program that exited; a pipeline reports its last command |
| `128 + sig` | a program killed by signal `sig` (the shell rebuilds this from the POSIX status word); also `fg` of a job that stops again |
| 0 | a builtin that succeeded; an empty line; a background job's start; a kernel command that ran |
| 1 | `cd` failed; a redirection's file could not be opened; a builtin's job does not exist |
| 2 | a syntax error (unclosed quote, a stray operator, an empty command in a pipeline, a redirection with no file, too many words) |
| 127 | not found; `fork` or `pipe` failed; a script that cannot be read |

`msh -c` and `msh file` exit with the last status. There is no `$?`; use `&&`
and `||`.

## Limits

| Limit | Value |
|---|---|
| line length | 1024 bytes |
| words and operators per line | 64 |
| words per command (argv) | 16 |
| bytes per word | 127 |
| commands per pipeline | 4 |
| `n>&m` per command | 4 |
| background jobs | 8 |
| path length | 255 |
| history | 32 lines of at most 256 bytes |

Going over one is an error message and status 2 (or 127 for a word too long to
pass to `execve`).

## Differences from POSIX sh

- No expansion at all: no variables, no `$?`, no command substitution, no
  globbing, no `~`. Quote removal is the only transformation of a word.
- No control flow, functions, subshells, here-documents or startup files.
- One search directory, `/bin`; `PATH` is not consulted.
- Unknown names fall through to the kernel console (`kcmd`).
- `cd` with no argument goes to `/`.
- `n>&m` is applied after all file redirections, whatever the written order.
- A pipeline has at most four commands.
- `echo` takes no options; `exit` with a non-number exits 0.
- `-l` and `-i` are accepted and change nothing.
- The prompt cannot be configured.
- History is not saved, and there is no `fc` or `!` history expansion.

## Examples

```sh
ls -l /bin | grep std | wc -l           # a three-command pipeline
cat notes.txt > copy.txt 2> errors.txt  # stdout and stderr to separate files
make-thing > build.log 2>&1             # both to one file
grep -i error /tmp/log || echo "no errors"
mkdir -p /tmp/a && cd /tmp/a && pwd     # cd here is the shell's own
LANG=C sort big.txt > sorted.txt        # an environment variable for one command
stdbench &                              # in the background; 'jobs', 'fg 1'
echo 'a "quoted" word' "and \"another\""
nvidia list                             # not in /bin: answered by the kernel
mikudo dmesg err -n 50                  # a privileged kernel command
msh -c 'echo one; echo two'             # one line, non-interactively
```

## Where the code is

| What | Where |
|---|---|
| Start-up, prompt, tokeniser, grammar, builtins, jobs, `exec` and the `kcmd` fall-through | `system/src/msh/main.rs` |
| The line editor and history | `system/src/msh/edit.rs` |
| The terminal that hosts it | `system/src/mayterm/main.rs` (`start_shell`) |
| Job control in the kernel | `kernel/syscall/adapters/tty.rs`, `kernel/io/tty/` |
| The `kcmd` call | `kernel/syscall/adapters/admin.rs` |
