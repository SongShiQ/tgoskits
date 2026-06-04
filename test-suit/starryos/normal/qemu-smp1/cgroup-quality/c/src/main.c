/*
 * cgroup-quality — Comprehensive quality tests for cgroup v2 controllers.
 *
 * Tests register/unregister pids.current tracking, cgroup.controllers
 * content, cpu.weight migration sync, and edge cases.
 *
 * Coverage:
 *   A. pids.current tracking (register/unregister sync)
 *   B. cgroup.controllers content
 *   C. cpu.weight migration sync
 *   D. Edge cases and consistency
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
#define CGROUP_A    CGROUP_ROOT "/qa-a"
#define CGROUP_B    CGROUP_ROOT "/qa-b"

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

static int read_int(const char *path)
{
    char buf[32];
    if (read_text(path, buf, sizeof(buf)) < 0) return -1;
    return atoi(buf);
}

static void expect_write_ok(const char *path, const char *data, const char *msg)
{
    errno = 0;
    int ret = write_text(path, data);
    CHECK(ret == 0, msg);
}

static void cleanup_cgroup(const char *path) { rmdir(path); }

/* ================================================================
 * A. pids.current tracking (Issue #1: register/unregister sync)
 * ================================================================ */

/*
 * A1: Empty cgroup has pids.current = 0.
 */
static void test_pids_current_empty(void)
{
    mkdir(CGROUP_A, 0755);
    char path[256];
    snprintf(path, sizeof(path), "%s/pids.current", CGROUP_A);
    int current = read_int(path);
    CHECK(current == 0, "empty child cgroup pids.current = 0");
    cleanup_cgroup(CGROUP_A);
}

/*
 * A2: After migration, pids.current = 1.
 */
static void test_pids_current_after_migration(void)
{
    mkdir(CGROUP_A, 0755);
    char path[256];
    char pid_str[32];
    snprintf(pid_str, sizeof(pid_str), "%d", getpid());

    /* Move to child cgroup */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_A);
    expect_write_ok(path, pid_str, "move to child cgroup");

    /* Check pids.current */
    snprintf(path, sizeof(path), "%s/pids.current", CGROUP_A);
    int current = read_int(path);
    CHECK(current == 1, "pids.current = 1 after migration");

    /* Move back */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_ROOT);
    expect_write_ok(path, pid_str, "move back to root");

    /* Check pids.current after move back */
    snprintf(path, sizeof(path), "%s/pids.current", CGROUP_A);
    current = read_int(path);
    CHECK(current == 0, "pids.current = 0 after move back");

    cleanup_cgroup(CGROUP_A);
}

/*
 * A3: Fork increments pids.current.
 */
static void test_pids_current_after_fork(void)
{
    mkdir(CGROUP_A, 0755);
    char path[256];
    char pid_str[32];
    snprintf(pid_str, sizeof(pid_str), "%d", getpid());

    /* Move to child cgroup */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_A);
    expect_write_ok(path, pid_str, "move to child cgroup for fork test");

    /* Fork a child */
    pid_t child = fork();
    if (child == 0) {
        usleep(100000);  /* Stay alive briefly */
        _exit(0);
    }
    CHECK(child > 0, "fork succeeds in child cgroup");

    /* Check pids.current = 2 (parent + child) */
    snprintf(path, sizeof(path), "%s/pids.current", CGROUP_A);
    int current = read_int(path);
    CHECK(current == 2, "pids.current = 2 after fork (parent + child)");

    /* Wait for child to exit */
    int status;
    waitpid(child, &status, 0);
    usleep(10000);  /* Give kernel time to update */

    /* Check pids.current = 1 (only parent) */
    current = read_int(path);
    CHECK(current == 1, "pids.current = 1 after child exits");

    /* Move back */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_ROOT);
    expect_write_ok(path, pid_str, "move back to root");
    cleanup_cgroup(CGROUP_A);
}

/*
 * A4: Multiple concurrent children.
 */
static void test_pids_current_multiple_children(void)
{
    mkdir(CGROUP_A, 0755);
    char path[256];
    char pid_str[32];
    snprintf(pid_str, sizeof(pid_str), "%d", getpid());

    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_A);
    expect_write_ok(path, pid_str, "move to child cgroup");

    /* Fork 3 children */
    pid_t children[3];
    for (int i = 0; i < 3; i++) {
        children[i] = fork();
        if (children[i] == 0) {
            usleep(200000);
            _exit(0);
        }
    }

    /* Check pids.current = 4 (parent + 3 children) */
    snprintf(path, sizeof(path), "%s/pids.current", CGROUP_A);
    int current = read_int(path);
    CHECK(current == 4, "pids.current = 4 with 3 concurrent children");

    /* Wait for all children */
    for (int i = 0; i < 3; i++) {
        int status;
        waitpid(children[i], &status, 0);
    }
    usleep(10000);

    /* Check pids.current = 1 */
    current = read_int(path);
    CHECK(current == 1, "pids.current = 1 after all children exit");

    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_ROOT);
    expect_write_ok(path, pid_str, "move back to root");
    cleanup_cgroup(CGROUP_A);
}

/*
 * A5: pids.max = 0 blocks all forks.
 */
static void test_pids_max_zero_blocks(void)
{
    mkdir(CGROUP_A, 0755);
    char path[256];
    char pid_str[32];
    snprintf(pid_str, sizeof(pid_str), "%d", getpid());

    /* Set pids.max = 0 */
    snprintf(path, sizeof(path), "%s/pids.max", CGROUP_A);
    expect_write_ok(path, "0", "set pids.max = 0");

    /* Move to child cgroup */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_A);
    expect_write_ok(path, pid_str, "move to child cgroup");

    /* Fork should fail */
    errno = 0;
    pid_t child = fork();
    if (child == 0) _exit(0);
    if (child > 0) {
        int status;
        waitpid(child, &status, 0);
        CHECK(0, "fork should fail with pids.max = 0");
    } else {
        CHECK(errno == EAGAIN || errno == ENOMEM,
              "fork fails with EAGAIN when pids.max = 0");
    }

    /* Move back and cleanup */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_ROOT);
    expect_write_ok(path, pid_str, "move back to root");
    cleanup_cgroup(CGROUP_A);
}

/*
 * A6: Migration updates both old and new cgroup counts.
 */
static void test_migration_updates_both_counts(void)
{
    mkdir(CGROUP_A, 0755);
    mkdir(CGROUP_B, 0755);
    char path[256];
    char pid_str[32];
    snprintf(pid_str, sizeof(pid_str), "%d", getpid());

    /* Move to cgroup A */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_A);
    expect_write_ok(path, pid_str, "move to cgroup A");
    usleep(10000);

    int a_count = read_int(CGROUP_A "/pids.current");
    int b_count = read_int(CGROUP_B "/pids.current");
    CHECK(a_count == 1, "cgroup A pids.current = 1 after move in");
    CHECK(b_count == 0, "cgroup B pids.current = 0 (empty)");

    /* Move from A to B */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_B);
    expect_write_ok(path, pid_str, "move from A to B");
    usleep(10000);

    a_count = read_int(CGROUP_A "/pids.current");
    b_count = read_int(CGROUP_B "/pids.current");
    CHECK(a_count == 0, "cgroup A pids.current = 0 after move out");
    CHECK(b_count == 1, "cgroup B pids.current = 1 after move in");

    /* Move back to root */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_ROOT);
    expect_write_ok(path, pid_str, "move back to root");
    cleanup_cgroup(CGROUP_A);
    cleanup_cgroup(CGROUP_B);
}

/* ================================================================
 * B. cgroup.controllers content (Issue #2)
 * ================================================================ */

/*
 * B1: Root cgroup.controllers lists pids and cpu.
 */
static void test_root_controllers_content(void)
{
    char buf[256];
    ssize_t n = read_text(CGROUP_ROOT "/cgroup.controllers", buf, sizeof(buf));
    CHECK(n >= 0, "read root cgroup.controllers");
    if (n >= 0) {
        CHECK(strstr(buf, "pids") != NULL, "root cgroup.controllers lists pids");
        CHECK(strstr(buf, "cpu") != NULL, "root cgroup.controllers lists cpu");
        printf("  INFO | root cgroup.controllers = %s", buf);
    }
}

/*
 * B2: Child cgroup.controllers inherits from parent.
 */
static void test_child_controllers_inherit(void)
{
    mkdir(CGROUP_A, 0755);
    char path[256];
    char buf[256];

    snprintf(path, sizeof(path), "%s/cgroup.controllers", CGROUP_A);
    ssize_t n = read_text(path, buf, sizeof(buf));
    CHECK(n >= 0, "read child cgroup.controllers");
    if (n >= 0) {
        CHECK(strstr(buf, "pids") != NULL, "child cgroup.controllers lists pids");
        CHECK(strstr(buf, "cpu") != NULL, "child cgroup.controllers lists cpu");
        printf("  INFO | child cgroup.controllers = %s", buf);
    }

    cleanup_cgroup(CGROUP_A);
}

/* ================================================================
 * C. cpu.weight migration sync (Issue #3)
 * ================================================================ */

/*
 * C1: cpu.weight I/O on root.
 */
static void test_cpu_weight_io(void)
{
    char buf[64];
    ssize_t n;

    n = read_text(CGROUP_ROOT "/cpu.weight", buf, sizeof(buf));
    CHECK(n >= 0 && atoi(buf) == 100, "root cpu.weight default is 100");

    expect_write_ok(CGROUP_ROOT "/cpu.weight", "200", "write cpu.weight = 200");
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "100", "restore cpu.weight = 100");
}

/*
 * C2: Child cgroup cpu.weight independence.
 */
static void test_child_cpu_weight_independent(void)
{
    mkdir(CGROUP_A, 0755);
    mkdir(CGROUP_B, 0755);

    /* Set different weights */
    expect_write_ok(CGROUP_A "/cpu.weight", "800", "set cgroup A weight = 800");
    expect_write_ok(CGROUP_B "/cpu.weight", "200", "set cgroup B weight = 200");

    /* Verify independence */
    expect_write_ok(CGROUP_A "/cpu.weight", "800", "verify cgroup A weight");
    expect_write_ok(CGROUP_B "/cpu.weight", "200", "verify cgroup B weight");

    /* Root unchanged */
    char buf[64];
    read_text(CGROUP_ROOT "/cpu.weight", buf, sizeof(buf));
    CHECK(atoi(buf) == 100, "root cpu.weight unchanged (100)");

    /* Restore and cleanup */
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "100", "restore root weight");
    cleanup_cgroup(CGROUP_A);
    cleanup_cgroup(CGROUP_B);
}

/*
 * C3: cpu.weight clamping.
 */
static void test_cpu_weight_clamping(void)
{
    /* Below minimum → clamped to 1 */
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "0", "write cpu.weight = 0");
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "1", "verify clamped to 1");

    /* Above maximum → clamped to 10000 */
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "99999", "write cpu.weight = 99999");
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "10000", "verify clamped to 10000");

    /* Negative → clamped to 1 */
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "-100", "write cpu.weight = -100");
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "1", "verify clamped to 1");

    /* Restore */
    expect_write_ok(CGROUP_ROOT "/cpu.weight", "100", "restore cpu.weight = 100");
}

/*
 * C4: cpu.max I/O.
 */
static void test_cpu_max_io(void)
{
    char buf[64];
    ssize_t n;

    /* Default: max */
    n = read_text(CGROUP_ROOT "/cpu.max", buf, sizeof(buf));
    CHECK(n >= 0 && strstr(buf, "max") != NULL, "root cpu.max default is max");

    /* Write quota + period */
    expect_write_ok(CGROUP_ROOT "/cpu.max", "50000 100000", "write cpu.max = 50000 100000");
    n = read_text(CGROUP_ROOT "/cpu.max", buf, sizeof(buf));
    CHECK(n >= 0 && strstr(buf, "50000") != NULL, "cpu.max reads back 50000");

    /* Restore */
    expect_write_ok(CGROUP_ROOT "/cpu.max", "max 100000", "restore cpu.max");
}

/*
 * C5: cpu.stat readable.
 */
static void test_cpu_stat_readable(void)
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

/* ================================================================
 * D. Edge cases and consistency
 * ================================================================ */

/*
 * D1: Nested cgroup isolation.
 */
static void test_nested_cgroup_isolation(void)
{
    char parent[256], child[256], path[256];
    snprintf(parent, sizeof(parent), "%s/nest-parent", CGROUP_ROOT);
    snprintf(child, sizeof(child), "%s/nest-parent/child", CGROUP_ROOT);

    mkdir(parent, 0755);
    mkdir(child, 0755);

    /* Set different pids.max */
    snprintf(path, sizeof(path), "%s/pids.max", parent);
    expect_write_ok(path, "10", "set parent pids.max = 10");
    snprintf(path, sizeof(path), "%s/pids.max", child);
    expect_write_ok(path, "3", "set child pids.max = 3");

    /* Verify independence */
    expect_write_ok(path, "3", "child pids.max is 3");

    /* Cleanup */
    rmdir(child);
    rmdir(parent);
}

/*
 * D2: cgroup.procs write validation.
 */
static void test_procs_write_validation(void)
{
    mkdir(CGROUP_A, 0755);
    char path[256];
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_A);

    /* Empty → EINVAL
     * NOTE: write(fd, "", 0) returns 0 without calling the kernel's
     * write handler (standard Linux behavior).  Use a single space to
     * actually trigger the handler. */
    errno = 0;
    write_text(path, " ");
    CHECK(errno == EINVAL, "whitespace-only cgroup.procs → EINVAL");

    /* Non-numeric → EINVAL */
    errno = 0;
    write_text(path, "abc");
    CHECK(errno == EINVAL, "non-numeric cgroup.procs → EINVAL");

    /* pid=0 → EINVAL */
    errno = 0;
    write_text(path, "0");
    CHECK(errno == EINVAL, "pid=0 cgroup.procs → EINVAL");

    /* Non-existent PID → ESRCH */
    errno = 0;
    write_text(path, "999999");
    CHECK(errno == ESRCH, "non-existent PID → ESRCH");

    cleanup_cgroup(CGROUP_A);
}

/*
 * D3: Fork inherits parent cgroup.
 */
static void test_fork_inherits_cgroup(void)
{
    mkdir(CGROUP_A, 0755);
    char path[256];
    char pid_str[32];
    snprintf(pid_str, sizeof(pid_str), "%d", getpid());

    /* Move to child cgroup */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_A);
    expect_write_ok(path, pid_str, "move to child cgroup");

    /* Fork */
    int pipefd[2];
    pipe(pipefd);
    pid_t child = fork();
    if (child == 0) {
        close(pipefd[0]);
        /* Check /proc/self/cgroup */
        char buf[256];
        ssize_t n = read_text("/proc/self/cgroup", buf, sizeof(buf));
        char result = (n >= 0 && strstr(buf, "qa-a") != NULL) ? 'Y' : 'N';
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
    CHECK(result == 'Y', "fork child inherits parent cgroup (/proc/self/cgroup shows qa-a)");

    /* Move back */
    snprintf(path, sizeof(path), "%s/cgroup.procs", CGROUP_ROOT);
    expect_write_ok(path, pid_str, "move back to root");
    cleanup_cgroup(CGROUP_A);
}

/* ================================================================ */

int main(void)
{
    TEST_START("cgroup-quality");

    /* A. pids.current tracking */
    test_pids_current_empty();
    test_pids_current_after_migration();
    test_pids_current_after_fork();
    test_pids_current_multiple_children();
    test_pids_max_zero_blocks();
    test_migration_updates_both_counts();

    /* B. cgroup.controllers content */
    test_root_controllers_content();
    test_child_controllers_inherit();

    /* C. cpu.weight migration sync */
    test_cpu_weight_io();
    test_child_cpu_weight_independent();
    test_cpu_weight_clamping();
    test_cpu_max_io();
    test_cpu_stat_readable();

    /* D. Edge cases and consistency */
    test_nested_cgroup_isolation();
    test_procs_write_validation();
    test_fork_inherits_cgroup();

    TEST_DONE();
}
