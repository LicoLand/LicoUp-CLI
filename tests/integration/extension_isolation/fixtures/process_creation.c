/* Bounded process-creation probe. After vfork the child executes only execve
 * and _exit, with arguments prepared before vfork. No Python VM is involved.
 * The same canonical binary is the parent and the explicitly allowed target. */
#include <errno.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
extern char **environ;

int main(int argc, char **argv) {
    if (argc != 2) return 64;
    if (strcmp(argv[1], "child") == 0) {
        alarm(5);
        FILE *f = fopen("child.executed", "w");
        if (!f) return 65;
        fputs("same-allowed-executable\n", f);
        return fclose(f) == 0 ? 0 : 66;
    }
    alarm(10);
    FILE *started = fopen("parent.started", "w");
    if (!started) return 67;
    fputs("initial-exec-allowed\n", started);
    if (fclose(started) != 0) return 68;
    char *child_args[] = {argv[0], "child", NULL};
    pid_t pid = -1;
    int error = 0;
    int wait_status = -1;
    int group = strcmp(argv[1], "fork-setpgid") == 0;
    if (strcmp(argv[1], "posix_spawn") == 0) {
        error = posix_spawn(&pid, argv[0], NULL, NULL, child_args, environ);
    } else if (strcmp(argv[1], "vfork-exec") == 0) {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        /* Deliberately probe the deprecated, still callable Darwin entry point. */
        pid = vfork();
#pragma clang diagnostic pop
        if (pid == 0) {
            execve(argv[0], child_args, environ);
            _exit(127);
        }
        if (pid < 0) error = errno;
    } else if (strcmp(argv[1], "fork-setsid") == 0 || group) {
        pid = fork();
        if (pid == 0) {
            alarm(5);
            if ((group ? setpgid(0, 0) : setsid()) == -1) _exit(126);
            execve(argv[0], child_args, environ);
            _exit(127);
        }
        if (pid < 0) error = errno;
    } else {
        return 69;
    }
    if (error == 0) {
        pid_t waited;
        do { waited = waitpid(pid, &wait_status, 0); } while (waited < 0 && errno == EINTR);
        if (waited != pid) return 70;
    }
    FILE *out = fopen("creation.outcome", "w");
    if (!out) return 71;
    fprintf(out, "%s %s %d %d\n", argv[1], error ? "denied" : "created", error,
            error ? -1 : (WIFEXITED(wait_status) ? WEXITSTATUS(wait_status) : 128));
    return fclose(out) == 0 ? 0 : 72;
}
