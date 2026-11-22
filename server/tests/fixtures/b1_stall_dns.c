#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <netdb.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* Linux fault injection: the async request may time out, but libc DNS keeps running. */
int getaddrinfo(const char *node, const char *service,
                const struct addrinfo *hints, struct addrinfo **res) {
    if (node && strcmp(node, "b1-stalled-dns.invalid") == 0) {
        const char *marker = getenv("EMUMET_TEST_DNS_MARKER");
        int fd = marker ? open(marker, O_CREAT | O_WRONLY | O_TRUNC, 0600) : -1;
        if (fd >= 0) {
            (void)write(fd, "started\n", 8);
            close(fd);
        }
        sleep(20);
        fd = marker ? open(marker, O_WRONLY | O_APPEND) : -1;
        if (fd >= 0) {
            (void)write(fd, "finished\n", 9);
            close(fd);
        }
        return EAI_AGAIN;
    }
    int (*real_getaddrinfo)(const char *, const char *, const struct addrinfo *,
                           struct addrinfo **) = dlsym(RTLD_NEXT, "getaddrinfo");
    return real_getaddrinfo(node, service, hints, res);
}
