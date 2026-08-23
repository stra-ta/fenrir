cmake_minimum_required(VERSION 3.25)
project(norn_fenrir_adapter VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# Find Norn
find_package(norn REQUIRED PATHS ${CMAKE_SOURCE_DIR}/../../../projects/norn NO_CMAKE_PATH)

# nlohmann_json for JSON parsing
find_package(nlohmann_json 3.11 REQUIRED)

# Threads for affinity
find_package(Threads REQUIRED)

add_executable(norn_fenrir_adapter
    fenrir_adapter.cpp
)

target_link_libraries(norn_fenrir_adapter PRIVATE
    norn::norn
    nlohmann_json::nlohmann_json
    Threads::Threads
)

target_compile_options(norn_fenrir_adapter PRIVATE
    -Wall -Wextra -Wpedantic -Wconversion -Wshadow -Werror
)

install(TARGETS norn_fenrir_adapter DESTINATION bin)