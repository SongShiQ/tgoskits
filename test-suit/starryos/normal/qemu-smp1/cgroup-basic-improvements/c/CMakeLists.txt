cmake_minimum_required(VERSION 3.10)
project(cgroup-basic-improvements C)

add_executable(cgroup-basic-improvements src/main.c)
target_link_libraries(cgroup-basic-improvements pthread)
