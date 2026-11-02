# Throwaway spike: Box2D v3.2.0 driven through its task interface by an external
# scheduler, state hashed at several worker counts. See README.md.
cmake_minimum_required(VERSION 3.22)

# C for the driver and for Box2D; C++ for enkiTS, the external scheduler.
project(box2d-worker-sweep LANGUAGES C CXX)

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

# enkiTS, the task scheduler the Box2D samples use, through its C interface.
set(ENKITS_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(ENKITS_BUILD_C_INTERFACE ON CACHE BOOL "" FORCE)
FetchContent_Declare(
  enkiTS
  GIT_REPOSITORY https://github.com/dougbinks/enkiTS.git
  GIT_TAG v1.12
  GIT_SHALLOW TRUE
)

FetchContent_MakeAvailable(box2d enkiTS)

# Box2D's shared scenes: the benchmark scenes and the falling hinges determinism
# scenario, compiled from the fetched tree so that the measurements use the
# author's own scenes and compare with the author's published numbers.
add_library(box2d_scenes STATIC
  ${box2d_SOURCE_DIR}/shared/benchmarks.c
  ${box2d_SOURCE_DIR}/shared/determinism.c
  ${box2d_SOURCE_DIR}/shared/human.c
  ${box2d_SOURCE_DIR}/shared/utils.c
)
target_include_directories(box2d_scenes PUBLIC ${box2d_SOURCE_DIR}/shared)
target_link_libraries(box2d_scenes PUBLIC box2d)
set_target_properties(box2d_scenes PROPERTIES C_STANDARD 17 C_STANDARD_REQUIRED YES)

add_executable(box2d-worker-sweep main.c)
set_target_properties(box2d-worker-sweep PROPERTIES C_STANDARD 17 C_STANDARD_REQUIRED YES)
target_link_libraries(box2d-worker-sweep PRIVATE box2d box2d_scenes enkiTS)
if (MSVC)
  target_compile_options(box2d-worker-sweep PRIVATE /W4)
else()
  target_compile_options(box2d-worker-sweep PRIVATE -Wall -Wextra -pedantic)
endif()

install(TARGETS box2d-worker-sweep RUNTIME DESTINATION bin)
