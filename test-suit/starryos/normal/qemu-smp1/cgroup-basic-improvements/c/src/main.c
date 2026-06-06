#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/types.h>
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

#define CGROUP2_PATH "/tmp/cg"

static int read_file(const char *path, char *buf, size_t size) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    ssize_t n = read(fd, buf, size - 1);
    close(fd);
    if (n < 0) return -1;
    buf[n] = '\0';
    return n;
}

static int write_file(const char *path, const char *data) {
    int fd = open(path, O_WRONLY);
    if (fd < 0) return -1;
    ssize_t n = write(fd, data, strlen(data));
    close(fd);
    return n >= 0 ? 0 : -1;
}

static void test_cgroup_type(void) {
    TEST_START("cgroup.type");
    
    char path[256];
    char buf[256];
    
    snprintf(path, sizeof(path), "%s/cgroup.type", CGROUP2_PATH);
    
    // Test 1: Read default type
    CHECK(read_file(path, buf, sizeof(buf)) >= 0, "Read cgroup.type");
    CHECK(strstr(buf, "domain") != NULL, "Default type is domain");
    
    // Test 2: Write valid type
    CHECK(write_file(path, "domain") == 0, "Write 'domain'");
    CHECK(read_file(path, buf, sizeof(buf)) >= 0, "Read after write");
    CHECK(strstr(buf, "domain") != NULL, "Type is still domain");
    
    // Test 3: Write invalid type (should fail)
    CHECK(write_file(path, "invalid") != 0, "Write 'invalid' fails");
}

static void test_cgroup_events(void) {
    TEST_START("cgroup.events");
    
    char path[256];
    char buf[256];
    
    snprintf(path, sizeof(path), "%s/cgroup.events", CGROUP2_PATH);
    
    // Test 1: Read events
    CHECK(read_file(path, buf, sizeof(buf)) >= 0, "Read cgroup.events");
    CHECK(strstr(buf, "populated") != NULL, "Has populated field");
    CHECK(strstr(buf, "frozen") != NULL, "Has frozen field");
}

static void test_cgroup_freeze(void) {
    TEST_START("cgroup.freeze");
    
    char path[256];
    char buf[256];
    
    snprintf(path, sizeof(path), "%s/cgroup.freeze", CGROUP2_PATH);
    
    // Test 1: Read default freeze state
    CHECK(read_file(path, buf, sizeof(buf)) >= 0, "Read cgroup.freeze");
    CHECK(strstr(buf, "0") != NULL, "Default freeze state is 0");
    
    // Test 2: Freeze
    CHECK(write_file(path, "1") == 0, "Freeze cgroup");
    CHECK(read_file(path, buf, sizeof(buf)) >= 0, "Read freeze state");
    CHECK(strstr(buf, "1") != NULL, "Freeze state is 1");
    
    // Test 3: Check events.frozen
    snprintf(path, sizeof(path), "%s/cgroup.events", CGROUP2_PATH);
    CHECK(read_file(path, buf, sizeof(buf)) >= 0, "Read events after freeze");
    CHECK(strstr(buf, "frozen 1") != NULL, "Events.frozen is 1");
    
    // Test 4: Thaw
    snprintf(path, sizeof(path), "%s/cgroup.freeze", CGROUP2_PATH);
    CHECK(write_file(path, "0") == 0, "Thaw cgroup");
    CHECK(read_file(path, buf, sizeof(buf)) >= 0, "Read thaw state");
    CHECK(strstr(buf, "0") != NULL, "Thaw state is 0");
}

static void test_cgroup_kill(void) {
    TEST_START("cgroup.kill");
    
    char path[256];
    
    snprintf(path, sizeof(path), "%s/cgroup.kill", CGROUP2_PATH);
    
    // Test 1: Kill empty cgroup
    CHECK(write_file(path, "1") == 0, "Kill empty cgroup succeeds");
    
    // Test 2: Invalid input
    CHECK(write_file(path, "2") != 0, "Invalid input fails");
}

static void test_procfs_cgroup(void) {
    TEST_START("procfs cgroup");
    
    char buf[256];
    
    // Test 1: Read /proc/self/cgroup
    CHECK(read_file("/proc/self/cgroup", buf, sizeof(buf)) >= 0, "Read /proc/self/cgroup");
    CHECK(strstr(buf, "0::") != NULL, "Has 0:: prefix");
}

int main(void) {
    // Mount cgroup2
    mkdir(CGROUP2_PATH, 0755);
    if (mount("cgroup2", CGROUP2_PATH, "cgroup2", 0, NULL) < 0) {
        // Already mounted?
        if (errno != EBUSY) {
            printf("Failed to mount cgroup2: %s\n", strerror(errno));
            return 1;
        }
    }
    
    test_cgroup_type();
    test_cgroup_events();
    test_cgroup_freeze();
    test_cgroup_kill();
    test_procfs_cgroup();
    
    TEST_DONE();
}
