# Throwaway spike: the Box2D half of the prepared collision geometry spike.
# Convex polygons and chains built from large inputs with Box2D v3.2.0, their
# written forms, and the worlds that read them. See ../README.md.
cmake_minimum_required(VERSION 3.22)

project(physics-prepared-geometry-box2d LANGUAGES C)

include(FetchContent)

# Deterministic math for every target of this project, as the Box2D build does
# for its own library: contraction of a * b + c into a fused multiply-add is
# the one value-changing transform some compilers enable by default (GCC on
# ARM64), and the driver computes its inputs in float.
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

add_executable(physics-prepared-geometry-box2d main.c)
set_target_properties(physics-prepared-geometry-box2d PROPERTIES C_STANDARD 17 C_STANDARD_REQUIRED YES C_EXTENSIONS OFF)
target_link_libraries(physics-prepared-geometry-box2d PRIVATE box2d)
if (MSVC)
  target_compile_options(physics-prepared-geometry-box2d PRIVATE /W4)
else()
  target_compile_options(physics-prepared-geometry-box2d PRIVATE -Wall -Wextra -pedantic)
endif()

install(TARGETS physics-prepared-geometry-box2d RUNTIME DESTINATION bin)
