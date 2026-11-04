#ifndef _SYS_WAIT_H
#define _SYS_WAIT_H

#include <_miku.h>
#include <sys/types.h>

#define WNOHANG 1
#define WEXITSTATUS(s) (((s) & 0xff00) >> 8)
#define WTERMSIG(s) ((s) & 0x7f)
#define WIFEXITED(s) (WTERMSIG(s) == 0)
#define WIFSIGNALED(s) (((signed char)(((s) & 0x7f) + 1) >> 1) > 0)

__MIKU_BEGIN
pid_t waitpid(pid_t, int *, int);
__MIKU_END

#endif
