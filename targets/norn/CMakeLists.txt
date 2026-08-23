cmake_minimum_required(VERSION 3.25)
project(norn_fenrir_adapter VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# nlohmann_json for JSON parsing
find_package(nlohmann_json 3.11 REQUIRED)

# Threads for affinity
find_package(Threads REQUIRED)

add_executable(norn_fenrir_adapter
    fenrir_adapter.cpp
)

# Find Norn: prefer the vendored, self-contained headers for CI; fall back
# to the local checkout otherwise. Norn is header-only, so an include dir is
# sufficient (no link target).
if(EXISTS "${CMAKE_SOURCE_DIR}/vendor/norn/norn/mpmc_queue.hpp")
  target_include_directories(norn_fenrir_adapter PRIVATE "${CMAKE_SOURCE_DIR}/vendor/norn")
else()
  find_package(norn REQUIRED PATHS ${CMAKE_SOURCE_DIR}/../../../projects/norn NO_CMAKE_PATH)
endif()

target_link_libraries(norn_fenrir_adapter PRIVATE
    nlohmann_json::nlohmann_json
    Threads::Threads
)

target_compile_options(norn_fenrir_adapter PRIVATE
    -Wall -Wextra -Wpedantic -Wconversion -Wshadow -Werror
)

install(TARGETS norn_fenrir_adapter DESTINATION bin)