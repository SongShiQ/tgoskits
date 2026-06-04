#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define MAX_LINE 256

static int test_file_exists(const char *path) {
    struct stat st;
    if (stat(path, &st) == 0) {
        printf("EXISTS: %s\n", path);
        return 0;
    }
    printf("NOT_FOUND: %s\n", path);
    return 1;
}

static int test_read_file(const char *path) {
    FILE *f = fopen(path, "r");
    if (!f) {
        printf("CANNOT_READ: %s\n", path);
        return 1;
    }
    char line[MAX_LINE];
    if (fgets(line, sizeof(line), f)) {
        printf("READ: %s -> %s", path, line);
    } else {
        printf("EMPTY: %s\n", path);
    }
    fclose(f);
    return 0;
}

static int test_mkdir(const char *path) {
    if (mkdir(path, 0755) == 0) {
        printf("MKDIR_OK: %s\n", path);
        return 0;
    }
    perror("mkdir");
    return 1;
}

static int test_write_file(const char *path, const char *content) {
    FILE *f = fopen(path, "w");
    if (!f) {
        printf("CANNOT_WRITE: %s\n", path);
        return 1;
    }
    fprintf(f, "%s", content);
    fclose(f);
    printf("WRITE_OK: %s\n", path);
    return 0;
}

int main(void) {
    int pass = 0, fail = 0;

    printf("=== CGROUP CAPABILITY PROBE ===\n\n");

    /* Test 1: /proc/self/cgroup */
    printf("[Test 1] /proc/self/cgroup\n");
    if (test_file_exists("/proc/self/cgroup") == 0) {
        test_read_file("/proc/self/cgroup");
        pass++;
    } else {
        fail++;
    }

    /* Test 2: /proc/self/ns/cgroup */
    printf("\n[Test 2] /proc/self/ns/cgroup\n");
    if (test_file_exists("/proc/self/ns/cgroup") == 0) {
        pass++;
    } else {
        fail++;
    }

    /* Test 3: /cgroup existence */
    printf("\n[Test 3] /cgroup\n");
    if (test_file_exists("/cgroup") == 0) {
        pass++;
    } else {
        fail++;
    }

    /* Test 4: cgroup.controllers */
    printf("\n[Test 4] /cgroup/cgroup.controllers\n");
    if (test_file_exists("/cgroup/cgroup.controllers") == 0) {
        test_read_file("/cgroup/cgroup.controllers");
        pass++;
    } else {
        fail++;
    }

    /* Test 5: cgroup.procs */
    printf("\n[Test 5] /cgroup/cgroup.procs\n");
    if (test_file_exists("/cgroup/cgroup.procs") == 0) {
        test_read_file("/cgroup/cgroup.procs");
        pass++;
    } else {
        fail++;
    }

    /* Test 6: mkdir test cgroup */
    printf("\n[Test 6] mkdir /cgroup/test-cgroup\n");
    if (test_mkdir("/cgroup/test-cgroup") == 0) {
        pass++;
    } else {
        fail++;
    }

    /* Test 7: write PID to cgroup.procs */
    printf("\n[Test 7] echo $$ > /cgroup/test-cgroup/cgroup.procs\n");
    {
        char path[MAX_LINE];
        snprintf(path, sizeof(path), "/cgroup/test-cgroup/cgroup.procs");
        if (test_file_exists(path) == 0) {
            char pid_str[32];
            snprintf(pid_str, sizeof(pid_str), "%d", getpid());
            if (test_write_file(path, pid_str) == 0) {
                pass++;
            } else {
                fail++;
            }
        } else {
            printf("SKIP: cgroup.procs not found (mkdir may have failed)\n");
            fail++;
        }
    }

    /* Test 8: pids.max exists */
    printf("\n[Test 8] pids.max in root cgroup\n");
    if (test_read_file("/cgroup/pids.max") == 0) {
        pass++;
    } else {
        fail++;
    }

    /* Test 9: pids.current exists */
    printf("\n[Test 9] pids.current in root cgroup\n");
    if (test_read_file("/cgroup/pids.current") == 0) {
        pass++;
    } else {
        fail++;
    }

    printf("\n=== RESULT: %d PASS, %d FAIL ===\n", pass, fail);
    return fail > 0 ? 1 : 0;
}
