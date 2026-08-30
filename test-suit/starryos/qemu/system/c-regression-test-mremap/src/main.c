/*
 * test_mremap.c — mremap 系统调用测试
 *
 * 覆盖范围:
 *   - 基本扩展/缩小/同大小
 *   - 原地扩展 (无 MREMAP_MAYMOVE 时尝试)
 *   - MREMAP_FIXED (移动到指定地址)
 *   - MREMAP_DONTUNMAP (移动但保留源映射)
 *   - 错误路径 (未对齐, new_size=0, 无效 flag, old_size 越界等)
 *   - 数据完整性验证
 *   - 重复扩展, FIXED+shrink/grow, 相邻 munmap 后扩展
 */

#define _GNU_SOURCE
#include "test_framework.h"
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <string.h>

#ifndef MREMAP_MAYMOVE
#define MREMAP_MAYMOVE 1
#endif
#ifndef MREMAP_FIXED
#define MREMAP_FIXED 2
#endif
#ifndef MREMAP_DONTUNMAP
#define MREMAP_DONTUNMAP 4
#endif

static void *raw_mremap(void *old_addr, size_t old_size, size_t new_size,
                        int flags, void *new_addr) {
    long ret = syscall(SYS_mremap, old_addr, old_size, new_size, flags, new_addr);
    if (ret == -1) return MAP_FAILED;
    return (void *)ret;
}

static int all_bytes_equal(const unsigned char *b, size_t n, unsigned char v) {
    for (size_t i = 0; i < n; i++)
        if (b[i] != v) return 0;
    return 1;
}

static int all_zero(const unsigned char *b, size_t n) {
    return all_bytes_equal(b, n, 0);
}

int main(void)
{
    const size_t PAGE = (size_t)sysconf(_SC_PAGE_SIZE);
    TEST_START("mremap");

    /* 1. 基本扩展 + 数据保持 + 新页面零初始化 */
    {
        void *p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for grow");
        if (p != MAP_FAILED) {
            memset(p, 0xAB, PAGE);
            void *p2 = mremap(p, PAGE, 2 * PAGE, MREMAP_MAYMOVE);
            CHECK(p2 != MAP_FAILED, "grow with MAYMOVE");
            if (p2 != MAP_FAILED) {
                unsigned char *b = (unsigned char *)p2;
                int ok = 1;
                for (size_t i = 0; i < PAGE; i++)
                    if (b[i] != 0xAB) { ok = 0; break; }
                CHECK(ok, "original data preserved");
                ok = 1;
                for (size_t i = PAGE; i < 2 * PAGE; i++)
                    if (b[i] != 0) { ok = 0; break; }
                CHECK(ok, "new pages zero-filled");
                munmap(p2, 2 * PAGE);
            } else {
                munmap(p, PAGE);
            }
        }
    }

    /* 2. 缩小返回原地址 */
    {
        void *p = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for shrink");
        if (p != MAP_FAILED) {
            memset(p, 0xCD, 4 * PAGE);
            void *p2 = mremap(p, 4 * PAGE, PAGE, 0);
            CHECK(p2 == p, "shrink returns same addr");
            if (p2 != MAP_FAILED) {
                CHECK(((unsigned char *)p2)[0] == 0xCD, "shrink data intact");
                munmap(p2, PAGE);
            } else {
                munmap(p, 4 * PAGE);
            }
        }
    }

    /* 3. 同大小无操作 */
    {
        void *p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for noop");
        if (p != MAP_FAILED) {
            CHECK(mremap(p, PAGE, PAGE, 0) == p, "same size returns same addr");
            munmap(p, PAGE);
        }
    }

    /* 4. 无 MAYMOVE 扩展: 原地成功或 ENOMEM */
    {
        void *p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for no-move");
        if (p != MAP_FAILED) {
            void *p2 = mremap(p, PAGE, 4 * PAGE, 0);
            if (p2 != MAP_FAILED) {
                CHECK(p2 == p, "no-move must be in-place");
                munmap(p2, 4 * PAGE);
            } else {
                CHECK(errno == ENOMEM, "no-move fails with ENOMEM");
                munmap(p, PAGE);
            }
        }
    }

    /* 5. MREMAP_FIXED 移动到指定地址 */
    {
        void *src = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        void *dst = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(src != MAP_FAILED && dst != MAP_FAILED, "mmap for FIXED");
        if (src != MAP_FAILED && dst != MAP_FAILED) {
            memset(src, 0xEE, PAGE);
            memset(dst, 0xBB, PAGE);
            void *r = raw_mremap(src, PAGE, PAGE,
                                 MREMAP_MAYMOVE | MREMAP_FIXED, dst);
            CHECK(r == dst, "FIXED moves to target");
            if (r == dst) {
                CHECK(((unsigned char *)dst)[0] == 0xEE, "data at target");
                munmap(dst, PAGE);
            } else {
                if (r != MAP_FAILED) munmap(r, PAGE);
                munmap(dst, PAGE);
            }
        }
    }

    /* 6. MREMAP_FIXED 不带 MAYMOVE -> EINVAL */
    {
        void *p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        void *dst = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p != MAP_FAILED && dst != MAP_FAILED) {
            CHECK_ERR(raw_mremap(p, PAGE, PAGE, MREMAP_FIXED, dst),
                      EINVAL, "FIXED without MAYMOVE");
            munmap(p, PAGE);
            munmap(dst, PAGE);
        }
    }

    /* 7. MREMAP_DONTUNMAP 基本功能 */
    {
        void *src = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(src != MAP_FAILED, "mmap for DONTUNMAP");
        if (src != MAP_FAILED) {
            memset(src, 0xFF, PAGE);
            void *dst = raw_mremap(src, PAGE, PAGE,
                                   MREMAP_MAYMOVE | MREMAP_DONTUNMAP, NULL);
            if (dst != MAP_FAILED) {
                CHECK(((unsigned char *)dst)[0] == 0xFF, "data moved");
                unsigned char sv = ((unsigned char *)src)[0];
                CHECK(sv == 0 || sv == 0xFF, "source accessible after DONTUNMAP");
                munmap(dst, PAGE);
                munmap(src, PAGE);
            } else {
                CHECK(errno == EINVAL || errno == ENOSYS,
                      "DONTUNMAP unsupported is ok");
                munmap(src, PAGE);
            }
        }
    }

    /* 8. DONTUNMAP 要求 MAYMOVE */
    {
        void *p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p != MAP_FAILED) {
            CHECK_ERR(raw_mremap(p, PAGE, PAGE, MREMAP_DONTUNMAP, NULL),
                      EINVAL, "DONTUNMAP without MAYMOVE");
            munmap(p, PAGE);
        }
    }

    /* 9. DONTUNMAP 要求 old_size == new_size */
    {
        void *p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p != MAP_FAILED) {
            CHECK_ERR(raw_mremap(p, PAGE, 2 * PAGE,
                                 MREMAP_MAYMOVE | MREMAP_DONTUNMAP, NULL),
                      EINVAL, "DONTUNMAP size mismatch");
            munmap(p, PAGE);
        }
    }

    /* 10-15. 错误用例 */
    {
        void *p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for errors");
        if (p != MAP_FAILED) {
            CHECK_ERR(mremap((char *)p + 1, PAGE, PAGE, MREMAP_MAYMOVE),
                      EINVAL, "unaligned addr");
            CHECK_ERR(mremap(p, PAGE, 0, MREMAP_MAYMOVE),
                      EINVAL, "zero new_size");
            CHECK_ERR(raw_mremap(p, PAGE, PAGE, 8, NULL),
                      EINVAL, "unknown flags (bit 3)");
            CHECK_ERR(raw_mremap(p, PAGE, PAGE, 0x100, NULL),
                      EINVAL, "unknown flags (bit 8)");
            CHECK_ERR(mremap(p, 2 * PAGE, 3 * PAGE, MREMAP_MAYMOVE),
                      EFAULT, "old_size exceeds VMA");
            CHECK_ERR(raw_mremap(p, 0, PAGE, MREMAP_MAYMOVE, NULL),
                      EINVAL, "old_size=0 private");
            munmap(p, PAGE);
        }
        CHECK_ERR(mremap((void *)0xDEAD0000, PAGE, PAGE, MREMAP_MAYMOVE),
                  EFAULT, "unmapped addr");
    }

    /* 16. FIXED 新旧范围重叠 -> EINVAL */
    {
        void *p = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p != MAP_FAILED) {
            CHECK_ERR(raw_mremap(p, 2 * PAGE, 2 * PAGE,
                                 MREMAP_MAYMOVE | MREMAP_FIXED,
                                 (char *)p + PAGE),
                      EINVAL, "FIXED overlap");
            munmap(p, 2 * PAGE);
        }
    }

    /* 17. FIXED + shrink */
    {
        void *src = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        void *dst = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (src != MAP_FAILED && dst != MAP_FAILED) {
            memset(src, 0xAA, 4 * PAGE);
            void *r = raw_mremap(src, 4 * PAGE, PAGE,
                                 MREMAP_MAYMOVE | MREMAP_FIXED, dst);
            CHECK(r == dst, "FIXED+shrink moves to target");
            if (r == dst) {
                CHECK(((unsigned char *)dst)[0] == 0xAA, "FIXED+shrink data");
                munmap(dst, PAGE);
            } else {
                if (r != MAP_FAILED) munmap(r, PAGE);
                munmap(dst, PAGE);
            }
        }
    }

    /* 18. FIXED + grow */
    {
        void *src = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        void *dst = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (src != MAP_FAILED && dst != MAP_FAILED) {
            memset(src, 0xBB, PAGE);
            void *r = raw_mremap(src, PAGE, 2 * PAGE,
                                 MREMAP_MAYMOVE | MREMAP_FIXED, dst);
            CHECK(r == dst, "FIXED+grow moves to target");
            if (r == dst) {
                CHECK(((unsigned char *)dst)[0] == 0xBB, "FIXED+grow data");
                CHECK(((unsigned char *)dst)[PAGE] == 0, "FIXED+grow new page zeroed");
                munmap(dst, 2 * PAGE);
            } else {
                if (r != MAP_FAILED) munmap(r, 2 * PAGE);
                munmap(dst, 2 * PAGE);
            }
        }
    }

    /* 19. 重复原地扩展 */
    {
        void *p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for repeated grow");
        if (p != MAP_FAILED) {
            ((unsigned char *)p)[0] = 0x11;
            void *p2 = mremap(p, PAGE, 2 * PAGE, MREMAP_MAYMOVE);
            if (p2 != MAP_FAILED) {
                ((unsigned char *)p2)[PAGE] = 0x22;
                void *p3 = mremap(p2, 2 * PAGE, 3 * PAGE, MREMAP_MAYMOVE);
                if (p3 != MAP_FAILED) {
                    CHECK(((unsigned char *)p3)[0] == 0x11, "repeated grow data[0]");
                    CHECK(((unsigned char *)p3)[PAGE] == 0x22, "repeated grow data[1]");
                    munmap(p3, 3 * PAGE);
                } else {
                    munmap(p2, 2 * PAGE);
                }
            } else {
                munmap(p, PAGE);
            }
        }
    }

    /* 20. 字节级数据完整性 */
    {
        void *p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for pattern");
        if (p != MAP_FAILED) {
            unsigned char *b = (unsigned char *)p;
            for (size_t i = 0; i < PAGE; i++)
                b[i] = (unsigned char)(i & 0xFF);
            void *p2 = mremap(p, PAGE, 3 * PAGE, MREMAP_MAYMOVE);
            if (p2 != MAP_FAILED) {
                b = (unsigned char *)p2;
                int ok = 1;
                for (size_t i = 0; i < PAGE; i++)
                    if (b[i] != (unsigned char)(i & 0xFF)) { ok = 0; break; }
                CHECK(ok, "byte pattern preserved");
                munmap(p2, 3 * PAGE);
            } else {
                munmap(p, PAGE);
            }
        }
    }

    /* 21. 页对齐 VMA 中段 mremap */
    {
        void *p = mmap(NULL, 3 * PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for mid-VMA move");
        if (p != MAP_FAILED) {
            unsigned char *b = (unsigned char *)p;
            memset(b, 0x11, PAGE);
            memset(b + PAGE, 0x22, PAGE);
            memset(b + 2 * PAGE, 0x33, PAGE);

            void *r = mremap(b + PAGE, PAGE, 2 * PAGE, MREMAP_MAYMOVE);
            CHECK(r != MAP_FAILED, "mid-VMA old_address can move");
            CHECK(b[0] == 0x11, "left fragment remains mapped after move");
            CHECK(b[2 * PAGE] == 0x33, "right fragment remains mapped after move");
            if (r != MAP_FAILED) {
                unsigned char *moved = (unsigned char *)r;
                CHECK(moved[0] == 0x22, "middle page data moved");
                CHECK(moved[PAGE] == 0, "expanded page is zero-filled");
                munmap(r, 2 * PAGE);
                munmap(p, PAGE);
                munmap(b + 2 * PAGE, PAGE);
            } else {
                munmap(p, 3 * PAGE);
            }
        }
    }

    /* 22. FIXED 失败后源映射应保持完整 */
    {
        void *src = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(src != MAP_FAILED, "mmap for failed move rollback");
        if (src != MAP_FAILED) {
            unsigned char *b = (unsigned char *)src;
            memset(b, 0x41, PAGE);
            memset(b + PAGE, 0x42, PAGE);
            memset(b + 2 * PAGE, 0x43, PAGE);
            memset(b + 3 * PAGE, 0x44, PAGE);

            errno = 0;
            void *r = raw_mremap(src, 4 * PAGE, PAGE,
                                 MREMAP_MAYMOVE | MREMAP_FIXED,
                                 NULL);
            CHECK(r == MAP_FAILED && errno != 0, "FIXED shrink to invalid target fails");
            CHECK(b[0] == 0x41, "failed move keeps first page mapped");
            CHECK(b[PAGE] == 0x42, "failed move keeps second page mapped");
            CHECK(b[2 * PAGE] == 0x43, "failed move keeps third page mapped");
            CHECK(b[3 * PAGE] == 0x44, "failed move keeps fourth page mapped");
            b[3 * PAGE] = 0x55;
            CHECK(b[3 * PAGE] == 0x55, "failed move source remains writable");
            munmap(src, 4 * PAGE);
        }
    }

    /* 23. 移动后重新分配源地址不应破坏目标页 */
    {
        void *src = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        void *dst = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(src != MAP_FAILED && dst != MAP_FAILED, "mmap for move lifetime");
        if (src != MAP_FAILED && dst != MAP_FAILED) {
            memset(src, 0x5A, PAGE);
            void *r = raw_mremap(src, PAGE, PAGE,
                                 MREMAP_MAYMOVE | MREMAP_FIXED, dst);
            CHECK(r == dst, "FIXED move for lifetime check");
            if (r == dst) {
                void *reused = mmap(src, PAGE, PROT_READ | PROT_WRITE,
                                    MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
                CHECK(reused == src, "old address can be reused");
                if (reused == src) {
                    memset(reused, 0xC3, PAGE);
                }
                ((unsigned char *)dst)[0] = 0x7E;
                CHECK(((unsigned char *)dst)[0] == 0x7E, "target remains writable");
                CHECK(((unsigned char *)dst)[PAGE - 1] == 0x5A,
                      "target keeps moved frame after source reuse");
                if (reused == src) munmap(reused, PAGE);
                munmap(dst, PAGE);
            } else {
                if (r != MAP_FAILED) munmap(r, PAGE);
                munmap(dst, PAGE);
                munmap(src, PAGE);
            }
        }
    }

    /* 24. 多页链式 grow + 全量内容校验
     * 板上 glibc 崩溃点是 mremap_chunk 断言，真实路径就是 mmapped chunk
     * 反复 grow。每轮 grow 后必须逐字节核对全部旧内容；步长 1→4→16→33
     * 覆盖原地扩、跨 VA 搬迁和奇数页尾。 */
    {
        const size_t steps[] = { 1, 4, 16, 33 };
        const size_t nsteps = sizeof(steps) / sizeof(steps[0]);
        size_t live = steps[0];
        const unsigned char pat = 0xA0;
        void *p = mmap(NULL, live * PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for chained grow");
        if (p != MAP_FAILED) {
            memset(p, pat, live * PAGE);
            int moved_ok = 1;
            for (size_t i = 1; i < nsteps; i++) {
                size_t old = live;
                void *p2 = mremap(p, live * PAGE, steps[i] * PAGE,
                                  MREMAP_MAYMOVE);
                CHECK(p2 != MAP_FAILED, "chained grow step");
                if (p2 == MAP_FAILED) {
                    moved_ok = 0;
                    break;
                }
                p = p2;
                live = steps[i];
                if (!all_bytes_equal((unsigned char *)p, old * PAGE, pat)) {
                    CHECK(0, "chained grow keeps ALL old bytes");
                    moved_ok = 0;
                    break;
                }
                memset((unsigned char *)p + old * PAGE, pat,
                       (live - old) * PAGE);
            }
            if (moved_ok)
                CHECK(1, "chained multi-page grow preserved content");
            munmap(p, live * PAGE);
        }
    }

    /* 25. 邻居映射完整性：一侧 grow 搬迁不得波及另一侧
     * 若内核原地扩时吞掉相邻 VMA，或搬迁时写错源，B 的内容会坏。 */
    {
        void *a = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        void *b = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(a != MAP_FAILED && b != MAP_FAILED, "mmap for neighbor case");
        if (a != MAP_FAILED && b != MAP_FAILED) {
            memset(a, 0x11, 2 * PAGE);
            memset(b, 0x22, 2 * PAGE);
            void *a2 = mremap(a, 2 * PAGE, 16 * PAGE, MREMAP_MAYMOVE);
            CHECK(a2 != MAP_FAILED, "neighbor A grows with MAYMOVE");
            if (a2 != MAP_FAILED) {
                CHECK(all_bytes_equal((unsigned char *)a2, 2 * PAGE, 0x11),
                      "grown A keeps old content");
                CHECK(all_bytes_equal((unsigned char *)b, 2 * PAGE, 0x22),
                      "neighbor B is untouched after A moved");
                ((unsigned char *)a2)[15 * PAGE] = 0x33;
                ((unsigned char *)b)[0] = 0x44;
                CHECK(((unsigned char *)b)[0] == 0x44,
                      "B stays writable after A moved");
                munmap(a2, 16 * PAGE);
            } else {
                munmap(a, 2 * PAGE);
            }
            munmap(b, 2 * PAGE);
        } else {
            if (a != MAP_FAILED) munmap(a, 2 * PAGE);
            if (b != MAP_FAILED) munmap(b, 2 * PAGE);
        }
    }

    /* 26. grow→shrink→grow 循环（realloc 收缩再扩张的真实节奏） */
    {
        void *p = mmap(NULL, 8 * PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for cycle case");
        if (p != MAP_FAILED) {
            size_t live = 8 * PAGE;
            int ok = 1;
            memset(p, 0x33, live);
            for (int round = 0; round < 4; round++) {
                void *p2 = mremap(p, live, 2 * PAGE, 0);
                CHECK(p2 == p, "cycle shrink stays in place");
                if (p2 != p) {
                    ok = 0;
                    break;
                }
                live = 2 * PAGE;
                if (!all_bytes_equal((unsigned char *)p, live, 0x33)) {
                    CHECK(0, "cycle shrink keeps head intact");
                    ok = 0;
                    break;
                }
                void *p3 = mremap(p, live, 8 * PAGE, MREMAP_MAYMOVE);
                CHECK(p3 != MAP_FAILED, "cycle grow back");
                if (p3 == MAP_FAILED) {
                    ok = 0;
                    break;
                }
                p = p3;
                live = 8 * PAGE;
                if (!all_bytes_equal((unsigned char *)p, 2 * PAGE, 0x33)) {
                    CHECK(0, "cycle regrown head intact");
                    ok = 0;
                    break;
                }
                if (!all_zero((unsigned char *)p + 2 * PAGE,
                              6 * PAGE)) {
                    CHECK(0, "cycle regrown tail zeroed");
                    ok = 0;
                    break;
                }
            }
            if (ok)
                CHECK(1, "grow/shrink cycles stable over 4 rounds");
            munmap(p, live);
        }
    }

    /* 27. 大跨度 grow（64 页→256 页）逐字节校验
     * 256KiB 旧内容跨整页搬迁，比对必须全量而非抽查首尾。 */
    {
        void *p = mmap(NULL, 64 * PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        CHECK(p != MAP_FAILED, "mmap for large grow");
        if (p != MAP_FAILED) {
            unsigned char *b = (unsigned char *)p;
            for (size_t i = 0; i < 64 * PAGE; i++)
                b[i] = (unsigned char)(i & 0xFF);
            void *p2 = mremap(p, 64 * PAGE, 256 * PAGE, MREMAP_MAYMOVE);
            CHECK(p2 != MAP_FAILED, "large grow 64->256 pages");
            if (p2 != MAP_FAILED) {
                b = (unsigned char *)p2;
                int ok = 1;
                for (size_t i = 0; i < 64 * PAGE; i++)
                    if (b[i] != (unsigned char)(i & 0xFF)) {
                        ok = 0;
                        break;
                    }
                CHECK(ok, "large grow keeps byte-exact pattern");
                CHECK(all_zero(b + 64 * PAGE, 192 * PAGE),
                      "large grow tail is zeroed");
                munmap(p2, 256 * PAGE);
            } else {
                munmap(p, 64 * PAGE);
            }
        }
    }

    TEST_DONE();
}
