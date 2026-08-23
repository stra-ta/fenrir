cmake_minimum_required(VERSION 3.25)
project(orda_fenrir_adapter VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# nlohmann_json for JSON parsing (confirmed available, >= 3.11).
find_package(nlohmann_json 3.11 REQUIRED)

# Threads (kept for symmetry with the runner/affinity path).
find_package(Threads REQUIRED)

add_executable(orda_fenrir_adapter
    fenrir_adapter.cpp
)

target_link_libraries(orda_fenrir_adapter PRIVATE
    nlohmann_json::nlohmann_json
    Threads::Threads
)

target_compile_options(orda_fenrir_adapter PRIVATE
    -Wall -Wextra -Wpedantic -Wshadow
)

install(TARGETS orda_fenrir_adapter DESTINATION bin)
