/* Sockets, over libc_miku's (lib/runtime/libmiku/libc/): AF_INET stream and
   datagram, AF_UNIX, raw ICMP. IPv4 only: the stack has no IPv6. Layouts are
   the Rust libc module's; the numbers are generated into <bits/socket.h> */
#ifndef _SYS_SOCKET_H
#define _SYS_SOCKET_H

#include <_miku.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <bits/socket.h>

typedef uint32_t socklen_t;
typedef uint16_t sa_family_t;

struct sockaddr { sa_family_t sa_family; char sa_data[14]; };
struct sockaddr_storage { sa_family_t ss_family; char __ss_pad[118]; unsigned long __ss_align; };
struct msghdr {
    void *msg_name;
    socklen_t msg_namelen;
    struct iovec *msg_iov;
    int msg_iovlen;
    void *msg_control;
    size_t msg_controllen;
    int msg_flags;
};

__MIKU_BEGIN
int socket(int, int, int);
int socketpair(int, int, int, int *);
int bind(int, const struct sockaddr *, socklen_t);
int listen(int, int);
int accept(int, struct sockaddr *, socklen_t *);
int accept4(int, struct sockaddr *, socklen_t *, int);
int connect(int, const struct sockaddr *, socklen_t);
ssize_t send(int, const void *, size_t, int);
ssize_t recv(int, void *, size_t, int);
ssize_t sendto(int, const void *, size_t, int, const struct sockaddr *, socklen_t);
ssize_t recvfrom(int, void *, size_t, int, struct sockaddr *, socklen_t *);
ssize_t sendmsg(int, const struct msghdr *, int);
ssize_t recvmsg(int, struct msghdr *, int);
int setsockopt(int, int, int, const void *, socklen_t);
int getsockopt(int, int, int, void *, socklen_t *);
int shutdown(int, int);
int getsockname(int, struct sockaddr *, socklen_t *);
int getpeername(int, struct sockaddr *, socklen_t *);
__MIKU_END

#endif
