

struct Point {
    int x;
    int y;
};

enum Color {
    RED = 0,
    GREEN = 1,
    BLUE = 2,
};


int add(int a, int b, int c) {
    return a + b + c;
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


int get_name(void) {
    return 42;
}
