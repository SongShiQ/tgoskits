# cgroup CPU Controller Integration - Test Plan

## Overview

This document defines the test cases for restoring CFS scheduler integration,
bandwidth throttling, and tick hook mechanism for the cgroup v2 CPU controller.

## Design Principles

1. **Test before implement**: Define exact expected behavior before coding
2. **Boundary conditions**: Test min/max values, edge cases, race conditions
3. **Real behavior verification**: Tests must verify actual scheduling behavior,
   not just file I/O
4. **No test-driven hacks**: Implementation must be correct, not just pass tests

---

## 1. CFS cgroup weight integration

### Test 1.1: weight combination formula

**Objective**: Verify `effective_weight = nice_weight * cgroup_weight / 100`

**Setup**:
- Create cgroup A with cpu.weight = 200
- Create cgroup B with cpu.weight = 50

**Action**:
- Fork two CPU-bound children (same nice=0)
- Child A in cgroup A, child B in cgroup B
- Each runs 100M iterations of busy loop
- Measure wall-clock time for each

**Expected**:
- Child A (weight=200) gets ~4x more CPU than child B (weight=50)
- Child A finishes ~4x faster than child B
- Tolerance: ±20% (scheduling is not perfectly precise)

**Boundary conditions**:
- weight=1 (minimum): should still make progress
- weight=10000 (maximum): should not overflow
- weight=100 (default): same as no cgroup weight

### Test 1.2: weight change propagation

**Objective**: Verify weight changes take effect immediately

**Setup**:
- Create cgroup with cpu.weight = 100
- Fork CPU-bound child in cgroup

**Action**:
- Child runs for 100ms
- Parent changes cpu.weight to 1000
- Child runs for another 100ms

**Expected**:
- Child gets more CPU time after weight increase
- No crash or deadlock during weight change

### Test 1.3: nice + cgroup weight combination

**Objective**: Verify combined weight calculation

**Setup**:
- Create cgroup A with cpu.weight = 200
- Create cgroup B with cpu.weight = 50

**Action**:
- Fork child A (nice=-5) in cgroup A
- Fork child B (nice=5) in cgroup B
- Both run CPU-bound workload

**Expected**:
- Combined weight A = NICE2WEIGHT_NEG[5] * 200 / 100
- Combined weight B = NICE2WEIGHT_POS[5] * 50 / 100
- A gets proportionally more CPU than B

---

## 2. Bandwidth throttling

### Test 2.1: quota enforcement

**Objective**: Verify cpu.max limits CPU usage

**Setup**:
- Create cgroup with cpu.max = "50000 100000" (50% quota)
- Fork CPU-bound child in cgroup

**Action**:
- Child runs for 1 second of wall time
- Measure child's actual CPU time

**Expected**:
- Child gets ~500ms CPU time (50% of 1 second)
- Child is throttled when quota exhausted
- Child resumes when period resets

**Boundary conditions**:
- quota=0: should block immediately
- quota=max: should not throttle
- period=1: very short period, rapid throttle/resume cycles

### Test 2.2: period reset

**Objective**: Verify throttled tasks resume after period

**Setup**:
- Create cgroup with cpu.max = "10000 100000" (10% quota)
- Fork CPU-bound child in cgroup

**Action**:
- Child runs until throttled
- Wait for period to expire
- Verify child resumes

**Expected**:
- Child is throttled after consuming 10ms CPU
- Child resumes after 100ms period
- Cycle repeats

### Test 2.3: throttle statistics

**Objective**: Verify cpu.stat counters are correct

**Setup**:
- Create cgroup with cpu.max = "50000 100000"
- Fork CPU-bound child in cgroup

**Action**:
- Child runs for 500ms
- Read cpu.stat

**Expected**:
- nr_periods > 0
- nr_throttled > 0
- throttled_usec > 0
- Values are monotonically increasing

### Test 2.4: multiple tasks in same cgroup

**Objective**: Verify quota is shared among tasks

**Setup**:
- Create cgroup with cpu.max = "50000 100000"
- Fork 4 CPU-bound children in same cgroup

**Action**:
- All children run concurrently
- Measure total CPU time

**Expected**:
- Total CPU time across all children ≈ 50% of wall time
- Each child gets ~12.5% CPU time (50% / 4)

---

## 3. Tick hook mechanism

### Test 3.1: tick hook registration

**Objective**: Verify tick hook is called on each timer tick

**Setup**:
- Register bandwidth_tick as tick hook
- Create counter to track calls

**Action**:
- Run scheduler_timer_tick 100 times

**Expected**:
- Counter reaches 100
- No panics or deadlocks

### Test 3.2: tick hook in atomic context

**Objective**: Verify tick hook is safe in IRQ context

**Setup**:
- Register tick hook

**Action**:
- Call scheduler_timer_tick from IRQ handler

**Expected**:
- No deadlocks (tick hook uses atomics, not locks)
- No panics

---

## 4. Process migration scheduler sync

### Test 4.1: cgroup weight sync on migration

**Objective**: Verify scheduler weight updates on cgroup change

**Setup**:
- Create cgroup A with cpu.weight = 100
- Create cgroup B with cpu.weight = 200
- Fork child in cgroup A

**Action**:
- Move child from cgroup A to cgroup B
- Read child's effective weight from scheduler

**Expected**:
- Child's cgroup_weight changes from 100 to 200
- Effective weight updates immediately

### Test 4.2: migration during throttling

**Objective**: Verify migration handles throttled state

**Setup**:
- Create cgroup A with cpu.max = "10000 100000"
- Fork child in cgroup A, let it get throttled

**Action**:
- Move child to cgroup B with cpu.max = "max 100000"

**Expected**:
- Child is unthrottled after migration
- Child can run immediately

---

## 5. Edge cases and race conditions

### Test 5.1: concurrent weight changes

**Objective**: Verify weight changes are thread-safe

**Setup**:
- Create cgroup with cpu.weight = 100
- Fork 4 children in cgroup

**Action**:
- Each child changes cpu.weight simultaneously
- No crashes or data races

### Test 5.2: cgroup deletion with running tasks

**Objective**: Verify cleanup when cgroup is deleted

**Setup**:
- Create cgroup with cpu.weight = 200
- Fork child in cgroup

**Action**:
- Delete cgroup while child is running

**Expected**:
- Child moves to parent cgroup
- No crash or memory leak

### Test 5.3: quota exhaustion with multiple tasks

**Objective**: Verify fair throttling when quota is exhausted

**Setup**:
- Create cgroup with cpu.max = "10000 100000"
- Fork 10 children in cgroup

**Action**:
- All children run concurrently

**Expected**:
- When quota exhausted, all children are throttled
- When period resets, all children resume
- No starvation

---

## Success Criteria

1. All existing cgroup-cpu tests pass (24 pass, 0 fail)
2. New tests for weight scheduling pass
3. New tests for bandwidth throttling pass
4. No deadlocks or panics in any test
5. No memory leaks
6. Local CI passes on all architectures (aarch64, loongarch64, riscv64, x86_64)
