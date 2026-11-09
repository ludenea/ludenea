# Throwaway spike: Box2D v3.2.0 driven by a scheduler shaped like the engine's
# frame pool, against the library's native scheduler, state hashed and step
# times measured at several worker counts. See README.md.
cmake_minimum_required(VERSION 3.22)

# C for the driver and for Box2D; C++ for the engine-shaped pool, whose
# threads, atomics and condition variables come from the C++ standard library
# so that the spike builds with the default compiler of every cell.
project(box2d-pool-sharing LANGUAGES C CXX)

include(FetchContent)

# Deterministic math for every target of this project, as the Box2D build does for
# its own library: contraction of a * b + c into a fused multiply-add is the one
# value-changing transform some compilers enable by default (GCC on ARM64), and
# the scene builders of Box2D's shared code compute positions in float.
if (CMAKE_C_COMPILER_ID MATCHES "GNU|Clang")
  if (CMAKE_C_COMPILER_FRONTEND_VARIANT STREQUAL "MSVC")
    add_compile_options(/clang:-ffp-contract=off)
  else()
    add_compile_options(-ffp-contract=off)
  endif()
endif()

set(BUILD_SHARED_LIBS OFF)

# Box2D at the studied tag. Its own build keeps samples, tests and benchmarks
# hidden from a FetchContent consumer, so only the library is built here.
FetchContent_Declare(
  box2d
  GIT_REPOSITORY https://github.com/erincatto/box2d.git
  GIT_TAG v3.2.0
  GIT_SHALLOW TRUE
)
FetchContent_MakeAvailable(box2d)

# Box2D's shared benchmark scenes, compiled from the fetched tree so that the
# measurements use the author's own scenes and compare with the worker sweep.
add_library(box2d_scenes STATIC
  ${box2d_SOURCE_DIR}/shared/benchmarks.c
  ${box2d_SOURCE_DIR}/shared/human.c
  ${box2d_SOURCE_DIR}/shared/utils.c
)
target_include_directories(box2d_scenes PUBLIC ${box2d_SOURCE_DIR}/shared)
target_link_libraries(box2d_scenes PUBLIC box2d)
set_target_properties(box2d_scenes PROPERTIES C_STANDARD 17 C_STANDARD_REQUIRED YES)

# The engine-shaped pool: a fixed team of workers claiming the library's tasks
# under a central counter, a bounded spin then a park, a finish that helps the
# library's pending tasks only, and a dedicated-subset variant whose other half
# runs a synthetic parallel pass.
add_library(engine_pool STATIC engine_pool.cpp engine_pool.h)
set_target_properties(engine_pool PROPERTIES CXX_STANDARD 17 CXX_STANDARD_REQUIRED YES)
target_include_directories(engine_pool PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
find_package(Threads REQUIRED)
target_link_libraries(engine_pool PUBLIC Threads::Threads)
if (MSVC)
  target_compile_options(engine_pool PRIVATE /W4)
else()
  target_compile_options(engine_pool PRIVATE -Wall -Wextra -pedantic)
endif()

add_executable(box2d-pool-sharing main.c)
set_target_properties(box2d-pool-sharing PROPERTIES C_STANDARD 17 C_STANDARD_REQUIRED YES)
target_link_libraries(box2d-pool-sharing PRIVATE box2d box2d_scenes engine_pool)
if (MSVC)
  target_compile_options(box2d-pool-sharing PRIVATE /W4)
else()
  target_compile_options(box2d-pool-sharing PRIVATE -Wall -Wextra -pedantic)
endif()

install(TARGETS box2d-pool-sharing RUNTIME DESTINATION bin)
