/* Writes a straight decode to compare recordings against: reference FILE FROM_FRAME FRAMES OUT */
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
  if (argc != 5) return 2;
  void *lib = dlopen("libopusfile.so.0", RTLD_NOW);
  if (!lib) return 1;
  void *(*open_file)(const char *, int *) = dlsym(lib, "op_open_file");
  int (*read_frames)(void *, float *, int) = dlsym(lib, "op_read_float_stereo");
  int (*seek)(void *, long long) = dlsym(lib, "op_pcm_seek");
  long long from = atoll(argv[2]), left = atoll(argv[3]);
  long long start = from > 144000 ? from - 144000 : 0, skip = from - start;
  int error, got;
  void *file = open_file(argv[1], &error);
  if (!file || seek(file, start) < 0) return 1;
  static float buffer[11520];
  while (skip > 0) {
    if ((got = read_frames(file, buffer, 2 * (skip < 5760 ? skip : 5760))) <= 0) return 1;
    skip -= got;
  }
  FILE *out = fopen(argv[4], "wb");
  if (!out) return 1;
  while (left > 0) {
    if ((got = read_frames(file, buffer, 2 * (left < 5760 ? left : 5760))) <= 0) break;
    fwrite(buffer, 8, got, out);
    left -= got;
  }
  return fclose(out) != 0;
}
