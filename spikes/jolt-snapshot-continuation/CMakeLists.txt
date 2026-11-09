# Throwaway spike: Jolt Physics v5.6.0 save states restored and stepped, state
# hashed against the uninterrupted run, the library driven by a job system
# implemented over a fixed pool of threads. See README.md.
cmake_minimum_required(VERSION 3.22)

project(jolt-snapshot-continuation LANGUAGES CXX)

include(FetchContent)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(BUILD_SHARED_LIBS OFF)

# Deterministic math for the spike's own code, as Jolt's cross-platform
# deterministic build sets for the library inside its own directory: precise
# floating point and no contraction into fused multiply-adds.
if (CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
  if (CMAKE_CXX_COMPILER_FRONTEND_VARIANT STREQUAL "MSVC")
    add_compile_options(/clang:-ffp-contract=off)
  else()
    add_compile_options(-ffp-contract=off)
  endif()
elseif (MSVC)
  add_compile_options(/fp:precise)
endif()

# Jolt's build options, pinned before the library is fetched: the cross-platform
# determinism option (the subject of the measurement; it also disables fused
# multiply-adds and selects precise floating point in the library's own flags),
# no link-time optimization (the library asks consumers to match it otherwise),
# the dynamic MSVC runtime (so that the spike and the library agree), and none
# of the extras the measurement does not need (debug renderer, profiler, object
# stream, GPU compute, floating-point exceptions, warnings as errors).
set(CROSS_PLATFORM_DETERMINISTIC ON CACHE BOOL "" FORCE)
set(INTERPROCEDURAL_OPTIMIZATION OFF CACHE BOOL "" FORCE)
set(USE_STATIC_MSVC_RUNTIME_LIBRARY OFF CACHE BOOL "" FORCE)
set(FLOATING_POINT_EXCEPTIONS_ENABLED OFF CACHE BOOL "" FORCE)
set(DEBUG_RENDERER_IN_DEBUG_AND_RELEASE OFF CACHE BOOL "" FORCE)
set(PROFILER_IN_DEBUG_AND_RELEASE OFF CACHE BOOL "" FORCE)
set(ENABLE_OBJECT_STREAM OFF CACHE BOOL "" FORCE)
set(ENABLE_ALL_WARNINGS OFF CACHE BOOL "" FORCE)
set(ENABLE_INSTALL OFF CACHE BOOL "" FORCE)
set(GENERATE_DEBUG_SYMBOLS OFF CACHE BOOL "" FORCE)
set(JPH_USE_DX12 OFF CACHE BOOL "" FORCE)
set(JPH_USE_VK OFF CACHE BOOL "" FORCE)
set(JPH_USE_MTL OFF CACHE BOOL "" FORCE)
set(JPH_USE_CPU_COMPUTE OFF CACHE BOOL "" FORCE)

# Jolt at the studied tag; its CMake project lives under Build/.
FetchContent_Declare(
  JoltPhysics
  GIT_REPOSITORY https://github.com/jrouwe/JoltPhysics.git
  GIT_TAG v5.6.0
  GIT_SHALLOW TRUE
  SOURCE_SUBDIR Build
)
FetchContent_MakeAvailable(JoltPhysics)

add_executable(jolt-snapshot-continuation main.cpp engine_job_system.cpp engine_job_system.h)
target_link_libraries(jolt-snapshot-continuation PRIVATE Jolt)
# The library is built without run-time type information; the spike matches it.
if (MSVC)
  target_compile_options(jolt-snapshot-continuation PRIVATE /W4 /GR-)
else()
  target_compile_options(jolt-snapshot-continuation PRIVATE -Wall -Wextra -fno-rtti)
endif()

install(TARGETS jolt-snapshot-continuation RUNTIME DESTINATION bin)
