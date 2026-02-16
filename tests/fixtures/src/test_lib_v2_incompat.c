

struct Point {
    int x;
    int y;
    int z;
};

enum Color {
    RED = 0,
    GREEN = 1,
    BLUE = 5,
};


long add(long a, long b) {
    return a + b;
}



struct Point make_point(int x, int y) {
    struct Point p = {x, y, 0};
    return p;
}

int get_color_value(enum Color c) {
    return (int)c;
}

const char* get_name(void) {
    return "v2_incompat";
}
