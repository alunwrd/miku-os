#ifndef _DIRENT_H
#define _DIRENT_H

#include <_miku.h>
#include <sys/types.h>

/* The getdents(111) wire struct; readdir returns a pointer into the batch */
struct dirent {
    char d_name[256];
    uint64_t d_ino;
    uint8_t d_type;
    uint8_t d_namlen;
    uint8_t d_reserved[6];
};

typedef struct _miku_DIR DIR;

__MIKU_BEGIN
DIR *opendir(const char *);
struct dirent *readdir(DIR *);
int closedir(DIR *);
void rewinddir(DIR *);
int dirfd(DIR *);
long telldir(DIR *);
void seekdir(DIR *, long);
__MIKU_END

#endif
