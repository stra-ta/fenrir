cmake_minimum_required(VERSION 3.25)
project(weir_fenrir_adapter VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# nlohmann_json for JSON parsing (same approach as the Norn adapter).
find_package(nlohmann_json 3.11 REQUIRED)

# Threads for std::thread / std::this_thread (timing mirror of Norn adapter).
find_package(Threads REQUIRED)

add_executable(weir_fenrir_adapter
    fenrir_adapter.cpp
)

target_link_libraries(weir_fenrir_adapter PRIVATE
    nlohmann_json::nlohmann_json
    Threads::Threads
)

target_compile_options(weir_fenrir_adapter PRIVATE
    -Wall -Wextra -Wpedantic -Wconversion -Wshadow -Werror
)

install(TARGETS weir_fenrir_adapter DESTINATION bin)
