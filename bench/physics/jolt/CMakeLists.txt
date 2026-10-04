# The Jolt side of the physics benchmark (docs/bench/physics.md). Builds Jolt from a checkout
# (JOLT_ROOT, default ~/Reference/JoltPhysics-v5.6.0, the latest release) with its own Build/CMakeLists.txt, so the library gets
# exactly the flags Jolt's PerformanceTest gets, and links jolt_bench against it.
#
#   cmake -S bench/physics/jolt -B <dir> -G Ninja -DCMAKE_BUILD_TYPE=Distribution \
#         [-DCROSS_PLATFORM_DETERMINISTIC=ON]
#   cmake --build <dir> -j 3
cmake_minimum_required(VERSION 3.20)
project(PocketJoltBench CXX)

set(JOLT_ROOT "$ENV{HOME}/Reference/JoltPhysics-v5.6.0" CACHE PATH "JoltPhysics checkout (release v5.6.0)")

# Distribution is Jolt's shipping configuration: Release optimization without the profiler and the
# debug renderer. Define it here too, since Jolt only defines it when it is the top-level project.
set(CMAKE_CXX_FLAGS_DISTRIBUTION "${CMAKE_CXX_FLAGS_RELEASE}")
set(CMAKE_EXE_LINKER_FLAGS_DISTRIBUTION "${CMAKE_EXE_LINKER_FLAGS_RELEASE}")

# Only the library: the samples, viewer and tests are not needed, nor GPU compute.
set(ENABLE_INSTALL OFF CACHE BOOL "" FORCE)
set(JPH_USE_DX12 OFF CACHE BOOL "" FORCE)
set(JPH_USE_VK OFF CACHE BOOL "" FORCE)
set(JPH_USE_MTL OFF CACHE BOOL "" FORCE)
add_subdirectory(${JOLT_ROOT}/Build ${CMAKE_BINARY_DIR}/Jolt EXCLUDE_FROM_ALL)

add_executable(jolt_bench jolt_bench.cpp)
target_compile_features(jolt_bench PRIVATE cxx_std_17)
target_include_directories(jolt_bench PRIVATE ${JOLT_ROOT}/PerformanceTest)
target_compile_definitions(jolt_bench PRIVATE JOLT_ASSETS="${JOLT_ROOT}/Assets/")
target_compile_options(jolt_bench PRIVATE -fno-rtti -fno-exceptions)
if (CROSS_PLATFORM_DETERMINISTIC)
	target_compile_options(jolt_bench PRIVATE -ffp-contract=off)
endif()
set_property(TARGET jolt_bench PROPERTY INTERPROCEDURAL_OPTIMIZATION_RELEASE ON)
set_property(TARGET jolt_bench PROPERTY INTERPROCEDURAL_OPTIMIZATION_DISTRIBUTION ON)
target_link_libraries(jolt_bench PRIVATE Jolt)
