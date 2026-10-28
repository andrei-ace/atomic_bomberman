#define _GNU_SOURCE
#define _XOPEN_SOURCE 700
#include <errno.h>
#include <fcntl.h>
#include <ftw.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#include "appimage-runtime.inc"

// Older AppImageLauncher versions detach the ELF prefix and interpose libc's
// readlink/realpath calls. A glibc entrypoint can resolve the original image;
// the embedded static runtime receives that path explicitly. Clear its AppImage
// marker before exec so the kernel's binfmt integration does not intercept it.
// Extract-and-run is automatic: no FUSE installation or mount is needed.
extern char **environ;
static volatile sig_atomic_t runtime_pid;

static void forward_signal(int number) {
    if (runtime_pid > 0) kill(-(pid_t)runtime_pid, number);
}
static int remove_entry(const char *path, const struct stat *info, int kind, struct FTW *walk) {
    (void)info; (void)kind; (void)walk;
    return remove(path);
}
static int is_image(const char *path) {
    unsigned char header[11];
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return 0;
    ssize_t length = read(fd, header, sizeof(header));
    close(fd);
    return length == sizeof(header) && !memcmp(header, "\177ELF", 4)
        && !memcmp(header + 8, "AI\002", 3);
}
static int resolve_image(char *image, const char *argument) {
    char candidate[PATH_MAX];
    ssize_t length = readlink("/proc/self/exe", candidate, sizeof(candidate) - 1);
    if (length > 0 && length < (ssize_t)sizeof(candidate) - 1) {
        candidate[length] = 0;
        if (realpath(candidate, image) && is_image(image)) return 1;
    }
    if (realpath(argument, image) && is_image(image)) return 1;
    const char *target = getenv("TARGET_APPIMAGE");
    return target && realpath(target, image) && is_image(image);
}
static int write_runtime(int fd) {
    size_t offset = 0;
    while (offset < sizeof(embedded_runtime)) {
        ssize_t length = write(fd, embedded_runtime + offset, sizeof(embedded_runtime) - offset);
        if (length < 0 && errno == EINTR) continue;
        if (length <= 0) return 0;
        offset += (size_t)length;
    }
    return pwrite(fd, "\0\0\0", 3, 8) == 3 && fchmod(fd, 0700) == 0;
}
int main(int argc, char **argv) {
    char image[PATH_MAX];
    if (!resolve_image(image, argv[0])) {
        fputs("Atomic Bomberman: cannot locate the original AppImage.\n", stderr);
        return 1;
    }
    int fd = memfd_create("atomic-bomberman-runtime", MFD_CLOEXEC);
    if (fd < 0 || !write_runtime(fd)) {
        perror("Atomic Bomberman: prepare AppImage runtime");
        if (fd >= 0) close(fd);
        return 1;
    }
    const char *base = getenv("TMPDIR");
    if (!base || base[0] != '/') base = "/tmp";
    char temporary[PATH_MAX];
    int count = snprintf(temporary, sizeof(temporary), "%s/atomic-appimage-XXXXXX", base);
    if (count < 0 || count >= (int)sizeof(temporary) || !mkdtemp(temporary)) {
        perror("Atomic Bomberman: create private runtime directory");
        close(fd);
        return 1;
    }
    if (setenv("TARGET_APPIMAGE", image, 1) || setenv("TMPDIR", temporary, 1)
        || setenv("APPIMAGE_EXTRACT_AND_RUN", "1", 1)) {
        perror("Atomic Bomberman: configure AppImage runtime");
        close(fd); rmdir(temporary); return 1;
    }
    // These explicit runtime operations retain their normal FUSE semantics.
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--appimage-mount")) unsetenv("APPIMAGE_EXTRACT_AND_RUN");
    }
    signal(SIGINT, forward_signal);
    signal(SIGTERM, forward_signal);
    signal(SIGHUP, forward_signal);
    pid_t child = fork();
    if (child == 0) {
        setpgid(0, 0);
        // execveat avoids libc hooks and retains the caller's exact arguments.
        syscall(SYS_execveat, fd, "", argv, environ, AT_EMPTY_PATH);
        perror("Atomic Bomberman: start AppImage runtime");
        _exit(127);
    }
    close(fd);
    int status = 0;
    if (child < 0) {
        perror("Atomic Bomberman: start AppImage process");
        rmdir(temporary); return 1;
    }
    runtime_pid = child;
    setpgid(child, child);
    pid_t waited;
    do { waited = waitpid(child, &status, 0); } while (waited < 0 && errno == EINTR);
    runtime_pid = 0;
    nftw(temporary, remove_entry, 32, FTW_DEPTH | FTW_PHYS);
    if (waited < 0) return 1;
    return WIFEXITED(status) ? WEXITSTATUS(status) : WIFSIGNALED(status) ? 128 + WTERMSIG(status) : 1;
}
