cmake_minimum_required(VERSION 3.25)
project(loki_fenrir_adapter VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# nlohmann_json for JSON parsing
find_package(nlohmann_json 3.11 REQUIRED)

# Threads (kept for parity with the Fenrir adapter contract)
find_package(Threads REQUIRED)

add_executable(loki_fenrir_adapter
    fenrir_adapter.cpp
)

target_link_libraries(loki_fenrir_adapter PRIVATE
    nlohmann_json::nlohmann_json
    Threads::Threads
)

target_compile_options(loki_fenrir_adapter PRIVATE
    -Wall -Wextra -Wpedantic
)

install(TARGETS loki_fenrir_adapter DESTINATION bin)
