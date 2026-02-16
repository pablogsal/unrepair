

extern int add(int a, int b);
extern int multiply(int a, int b);

struct Point {
    int x;
    int y;
};
extern struct Point make_point(int x, int y);

extern const char* get_name(void);

int extension_func(void) {
    int sum = add(1, 2);
    int prod = multiply(3, 4);
    struct Point p = make_point(5, 6);
    const char* name = get_name();
    return sum + prod + p.x + p.y;
}
