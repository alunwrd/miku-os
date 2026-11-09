/* Names: libc_miku's getaddrinfo (lib/runtime/libmiku/libc/netdb.rs): a literal,
   localhost, /etc/hosts, then DNS to the lease's resolver. IPv4 only */
#ifndef _NETDB_H
#define _NETDB_H

#include <_miku.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <bits/netdb.h>

struct addrinfo {
    int ai_flags;
    int ai_family;
    int ai_socktype;
    int ai_protocol;
    socklen_t ai_addrlen;
    struct sockaddr *ai_addr;
    char *ai_canonname;
    struct addrinfo *ai_next;
};

__MIKU_BEGIN
int getaddrinfo(const char *, const char *, const struct addrinfo *, struct addrinfo **);
void freeaddrinfo(struct addrinfo *);
const char *gai_strerror(int);
__MIKU_END

#endif
