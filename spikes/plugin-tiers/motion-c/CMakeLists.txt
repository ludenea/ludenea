# The motion pass as a C dynamic library, built at run time by the plugin tiers
# spike with the platform's default C compiler. MOTION_PERMITTED selects the
# permitted floating-point configuration (no contraction); off, the compiler's
# defaults decide, which is the control the spike measures.
cmake_minimum_required(VERSION 3.16)
project(motion_c C)

option(MOTION_PERMITTED "Build with the permitted floating-point configuration (no contraction)" OFF)

add_library(motion_c SHARED motion.c)
set_target_properties(motion_c PROPERTIES C_STANDARD 11 C_EXTENSIONS OFF)

if(MOTION_PERMITTED)
  if(CMAKE_C_COMPILER_ID MATCHES "MSVC")
    target_compile_options(motion_c PRIVATE /fp:strict)
  else()
    target_compile_options(motion_c PRIVATE -ffp-contract=off)
  endif()
endif()

# The compiler and the flags actually used, for the spike's report.
set(MOTION_FLAGS "${CMAKE_C_FLAGS} ${CMAKE_C_FLAGS_RELEASE}")
get_target_property(MOTION_OPTIONS motion_c COMPILE_OPTIONS)
if(MOTION_OPTIONS)
  string(APPEND MOTION_FLAGS " ${MOTION_OPTIONS}")
endif()
file(WRITE "${CMAKE_BINARY_DIR}/motion-build.txt"
  "compiler=${CMAKE_C_COMPILER_ID} ${CMAKE_C_COMPILER_VERSION}\nflags=${MOTION_FLAGS}\npermitted=${MOTION_PERMITTED}\n")

install(TARGETS motion_c LIBRARY DESTINATION lib RUNTIME DESTINATION lib)
