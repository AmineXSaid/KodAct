#include <stdio.h>

/* Prints how many words the file named by its argument holds: runs of characters that are not
   spaces, tabs or newlines. */
int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s FILE\n", argv[0]);
        return 2;
    }
    FILE *f = fopen(argv[1], "r");
    if (!f) {
        perror(argv[1]);
        return 1;
    }
    long words = 0;
    int c, prev = ' ';
    while ((c = fgetc(f)) != EOF) {
        if (c == ' ' && prev != ' ')
            words++;
        prev = c;
    }
    if (prev != ' ')
        words++;
    fclose(f);
    printf("%ld\n", words);
    return 0;
}
