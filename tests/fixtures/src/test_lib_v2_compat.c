
#include <stddef.h>

struct Point {
    int x;
    int y;
};

enum Color {
    RED = 0,
    GREEN = 1,
    BLUE = 2,
    YELLOW = 3,
};

int add(int a, int b) {
    return a + b;
}

int multiply(int a, int b) {
    return a * b;
}

struct Point make_point(int x, int y) {
    struct Point p = {x, y};
    return p;
}

int get_color_value(enum Color c) {
    return (int)c;
}

const char* get_name(void) {
    return "v2_compat";
}


int subtract(int a, int b) {
    return a - b;
}
