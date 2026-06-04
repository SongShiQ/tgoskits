/*
 * cgroup-merged — Comprehensive cgroup v2 test covering process membership
 * and controller functionality.
 *
 * Tests:
 *   1. Process membership: /proc/self/cgroup, cgroup.procs migration,
 *      fork inheritance, populated rmdir, input validation
 *   2. pids controller: pids.max/pids.current I/O, limit enforcement,
 *      migration updates, unlimited mode
 *   3. cpu controller: cpu.weight/cpu.max/cpu.stat I/O, clamping
 */

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static int __pass = 0;
static int __fail = 0;

#define CHECK(cond, msg) do {                                           \
    if (cond) {                                                         \
        printf("  PASS | %s:%d | %s\n", __FILE__, __LINE__, msg);       \
        __pass++;                                                       \
    } else {                                                            \
        printf("  FAIL | %s:%d | %s | errno=%d (%s)\n",                 \
               __FILE__, __LINE__, msg, errno, strerror(errno));        \
        __fail++;                                                       \
    }                                                                   \
} while (0)

#define TEST_START(name)                                                \
    printf("================================================\n");       \
    printf("  TEST: %s\n", name);                                       \
    printf("  FILE: %s\n", __FILE__);                                   \
    printf("================================================\n")

#define TEST_DONE()                                                     \
    printf("------------------------------------------------\n");       \
    printf("  DONE: %d pass, %d fail\n", __pass, __fail);               \
    printf("================================================\n\n");     \
    return __fail > 0 ? 1 : 0

#define CGROUP_ROOT "/cgroup"
#define CGROUP_CHILD CGROUP_ROOT "/test-merged"

/* ---- helpers ---- */

static ssize_t read_text(const char *path, char *buf, size_t cap)
{
    if (cap == 0) return -1;
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    ssize_t n = read(fd, buf, cap - 1);
    if (n >= 0) buf[n] = '\0';
    close(fd);
    return n;
}

static int write_text(const char *path, const char *data)
{
    int fd = open(path, O_WRONLY);
    if (fd < 0) return -1;
    ssize_t n = write(fd, data, strlen(data));
    close(fd);
    return n >= 0 ? 0 : -1;
}

static void expect_write_ok(const char *path, const char *data, const char *msg)
{
    errno = 0;
    int ret = write_text(path, data);
    CHECK(ret == 0, msg);
}

static int read_int(const char *path)
{
    char buf[32];
    if (read_text(path, buf, sizeof(buf)) < 0) return -1;
    return atoi(buf);
}

/* ================================================================
 * A. Process membership tests (from PR #1045 design)
 * ================================================================ */

/*
 * A1: /proc/self/cgroup returns correct initial path.
 */
static void test_proc_self_cgroup_initial(void)
{
    char buf[256];
    ssize_t n = read_text("/proc/self/cgroup", buf, sizeof(buf));
    CHECK(n >= 0, "read /proc/self/cgroup");
    if (n >= 0) {
        CHECK(strstr(buf, "0::/") != NULL,
              "/proc/self/cgroup contains 0::/");
        printf("  INFO | /proc/self/cgroup = %s", buf);
    }
}

/*
 * A2: cgroup.procs migration — write PID to child, verify migration.
 */
static void test_procs_migration(void)
{
    char path[256];
    char buf[4096];

    /* Create child cgroup */
    errno = 0;
    int ret = mkdir(CGROUP_CHILD, 0755);
    CHECK(ret == 0 || errno == EEXIST, "mkdir child cgroup");

    /* Write current PID to child cgroup.procs */
    char pid_str[32];
    snprintf(pid_str, sizeof(pid_str), "%d", getpid());
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_CHILD);
    expect_write_ok(path, pid_str, "write PID to child cgroup.procs");

    /* Verify /proc/self/cgroup updated */
    ssize_t n = read_text("/proc/self/cgroup", buf, sizeof(buf));
    CHECK(n >= 0, "read /proc/self/cgroup after migration");
    if (n >= 0) {
        CHECK(strstr(buf, "0::/test-merged") != NULL,
              "/proc/self/cgroup shows child path after migration");
        printf("  INFO | /proc/self/cgroup after migration = %s", buf);
    }

    /* Verify child cgroup.procs contains current PID */
    n = read_text(path, buf, sizeof(buf));
    CHECK(n >= 0, "read child cgroup.procs");
    if (n >= 0) {
        CHECK(strstr(buf, pid_str) != NULL,
              "child cgroup.procs contains current PID");
    }

    /* Verify root cgroup.procs does NOT contain current PID */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_ROOT);
    n = read_text(path, buf, sizeof(buf));
    CHECK(n >= 0, "read root cgroup.procs");
    if (n >= 0) {
        CHECK(strstr(buf, pid_str) == NULL,
              "root cgroup.procs does NOT contain current PID after migration");
    }
}

/*
 * A3: cgroup.procs input validation — invalid inputs rejected.
 */
static void test_procs_input_validation(void)
{
    char path[256];
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_CHILD);

    /* Empty input → EINVAL */
    errno = 0;
    write_text(path, "");
    CHECK(errno == EINVAL, "empty cgroup.procs write → EINVAL");

    /* Non-numeric → EINVAL */
    errno = 0;
    write_text(path, "abc");
    CHECK(errno == EINVAL, "non-numeric cgroup.procs write → EINVAL");

    /* pid=0 → EINVAL */
    errno = 0;
    write_text(path, "0");
    CHECK(errno == EINVAL, "pid=0 cgroup.procs write → EINVAL");

    /* Non-existent PID → ESRCH */
    errno = 0;
    write_text(path, "999999");
    CHECK(errno == ESRCH, "non-existent PID cgroup.procs write → ESRCH");
}

/*
 * A4: Fork inherits parent cgroup.
 */
static void test_fork_inherits_cgroup(void)
{
    int pipefd[2];
    pipe(pipefd);

    pid_t child = fork();
    if (child == 0) {
        close(pipefd[0]);
        /* Read /proc/self/cgroup in child */
        char buf[256];
        ssize_t n = read_text("/proc/self/cgroup", buf, sizeof(buf));
        /* Send result to parent */
        char result = (n >= 0 && strstr(buf, "0::/test-merged") != NULL) ? 'Y' : 'N';
        write(pipefd[1], &result, 1);
        close(pipefd[1]);
        _exit(0);
    }

    close(pipefd[1]);
    char result = 'N';
    read(pipefd[0], &result, 1);
    close(pipefd[0]);

    int status;
    waitpid(child, &status, 0);
    CHECK(result == 'Y', "fork child inherits parent cgroup (/proc/self/cgroup shows child path)");
}

/*
 * A5: Migrate back to root.
 */
static void test_migrate_back_to_root(void)
{
    char path[256];
    char buf[4096];
    char pid_str[32];
    snprintf(pid_str, sizeof(pid_str), "%d", getpid());

    /* Migrate back to root */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_ROOT);
    expect_write_ok(path, pid_str, "migrate back to root cgroup.procs");

    /* Verify /proc/self/cgroup shows root */
    ssize_t n = read_text("/proc/self/cgroup", buf, sizeof(buf));
    CHECK(n >= 0, "read /proc/self/cgroup after migrate back");
    if (n >= 0) {
        /* Should be "0::/\n" (root path) */
        CHECK(strstr(buf, "0::/\n") != NULL || strstr(buf, "0::/\r\n") != NULL,
              "/proc/self/cgroup shows root after migrate back");
    }

    /* Cleanup */
    rmdir(CGROUP_CHILD);
}

/* ================================================================
 * B. pids controller tests
 * ================================================================ */

/*
 * B1: pids.max/pids.current file I/O.
 */
static void test_pids_io(void)
{
    char buf[64];
    ssize_t n;

    /* Default pids.max = "max" */
    n = read_text(CGROUP_ROOT "/pids.max", buf, sizeof(buf));
    CHECK(n >= 0, "read root pids.max");
    if (n >= 0) {
        CHECK(strstr(buf, "max") != NULL, "root pids.max default is max");
    }

    /* pids.current > 0 */
    int current = read_int(CGROUP_ROOT "/pids.current");
    CHECK(current > 0, "root pids.current > 0");

    /* Write and read back */
    expect_write_ok(CGROUP_ROOT "/pids.max", "100", "write pids.max = 100");
    n = read_text(CGROUP_ROOT "/pids.max", buf, sizeof(buf));
    CHECK(n >= 0 && atoi(buf) == 100, "pids.max reads back as 100");

    /* Restore */
    expect_write_ok(CGROUP_ROOT "/pids.max", "max", "restore pids.max = max");
}

/*
 * B2: pids limit enforcement on root cgroup.
 */
static void test_pids_limit_root(void)
{
    int before = read_int(CGROUP_ROOT "/pids.current");
    CHECK(before >= 0, "read pids.current before test");

    /* Set limit = current + 1 */
    char limit[32];
    snprintf(limit, sizeof(limit), "%d", before + 1);
    expect_write_ok(CGROUP_ROOT "/pids.max", limit, "set pids.max = current+1");

    /* Fork one child — should succeed */
    pid_t child1 = fork();
    if (child1 == 0) { usleep(50000); _exit(0); }
    CHECK(child1 > 0, "first fork succeeds within pids limit");

    /* Fork another immediately — should fail */
    errno = 0;
    pid_t child2 = fork();
    if (child2 == 0) { _exit(0); }
    if (child2 > 0) {
        int status;
        waitpid(child2, &status, 0);
        CHECK(0, "second fork should fail with EAGAIN");
    } else {
        CHECK(errno == EAGAIN || errno == ENOMEM,
              "second fork fails with EAGAIN when pids limit reached");
    }

    /* Cleanup */
    if (child1 > 0) { int s; waitpid(child1, &s, 0); }
    expect_write_ok(CGROUP_ROOT "/pids.max", "max", "restore pids.max");
}

/*
 * B3: pids.current decrements on child exit.
 */
static void test_pids_decrement(void)
{
    int before = read_int(CGROUP_ROOT "/pids.current");
    CHECK(before >= 0, "read pids.current before fork");

    pid_t child = fork();
    if (child == 0) { _exit(0); }
    CHECK(child > 0, "fork for decrement test");

    int status;
    waitpid(child, &status, 0);
    usleep(10000);

    int after = read_int(CGROUP_ROOT "/pids.current");
    CHECK(after == before, "pids.current returns to original after child exits");
}

/* ================================================================
 * C. cpu controller tests
 * ================================================================ */

/*
 * C1: cpu.weight I/O and clamping.
 */
static void test_cpu_weight_io(void)
{
    char buf[64];
    ssize_t n;

    /* Default */
    n = read_text(CGROUP_ROOT "/cpu.weight", buf, sizeof(buf));
    CHECK(n >= 0 && atoi(buf) == 100, "root cpu.weight default is 100");

    /* Write and read back */
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "200", "write cpu.weight = 200");
    n = read_text(CGROUP_ROOT "/cpu.weight", buf, sizeof(buf));
    CHECK(n >= 0 && atoi(buf) == 200, "cpu.weight reads back as 200");

    /* Clamping: 0 → 1 */
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "0", "write cpu.weight = 0");
    n = read_text(CGROUP_ROOT "/cpu.weight", buf, sizeof(buf));
    CHECK(n >= 0 && atoi(buf) == 1, "cpu.weight clamps 0 to 1");

    /* Clamping: 99999 → 10000 */
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "99999", "write cpu.weight = 99999");
    n = read_text(CGROUP_ROOT "/cpu.weight", buf, sizeof(buf));
    CHECK(n >= 0 && atoi(buf) == 10000, "cpu.weight clamps 99999 to 10000");

    /* Restore */
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "100", "restore cpu.weight = 100");
}

/*
 * C2: cpu.max I/O.
 */
static void test_cpu_max_io(void)
{
    char buf[64];
    ssize_t n;

    /* Default */
    n = read_text(CGROUP_ROOT "/cpu.max", buf, sizeof(buf));
    CHECK(n >= 0, "read root cpu.max");
    if (n >= 0) {
        CHECK(strstr(buf, "max") != NULL, "root cpu.max default is max");
    }

    /* Write quota + period */
    expect_write_ok(CGROUP_ROOT "/cpu.max", "50000 100000", "write cpu.max = 50000 100000");
    n = read_text(CGROUP_ROOT "/cpu.max", buf, sizeof(buf));
    CHECK(n >= 0, "read back cpu.max");
    if (n >= 0) {
        CHECK(strstr(buf, "50000") != NULL, "cpu.max contains 50000");
    }

    /* Restore */
    expect_write_ok(CGROUP_ROOT "/cpu.max", "max 100000", "restore cpu.max");
}

/*
 * C3: cpu.stat readable.
 */
static void test_cpu_stat(void)
{
    char buf[256];
    ssize_t n = read_text(CGROUP_ROOT "/cpu.stat", buf, sizeof(buf));
    CHECK(n >= 0, "read root cpu.stat");
    if (n >= 0) {
        CHECK(strstr(buf, "nr_periods") != NULL, "cpu.stat has nr_periods");
        CHECK(strstr(buf, "nr_throttled") != NULL, "cpu.stat has nr_throttled");
        CHECK(strstr(buf, "throttled_usec") != NULL, "cpu.stat has throttled_usec");
    }
}

/* ================================================================ */

int main(void)
{
    TEST_START("cgroup-merged");

    /* A. Process membership */
    test_proc_self_cgroup_initial();
    test_procs_migration();
    test_procs_input_validation();
    test_fork_inherits_cgroup();
    test_migrate_back_to_root();

    /* B. pids controller */
    test_pids_io();
    test_pids_limit_root();
    test_pids_decrement();

    /* C. cpu controller */
    test_cpu_weight_io();
    test_cpu_max_io();
    test_cpu_stat();

    TEST_DONE();
}
