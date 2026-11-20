# CMG sample files

From [CMG_raylib](https://www.cem.pp.ua/Akumarin/CMG_raylib) by Akumarin, MIT licensed (see
`LICENSE-CMG_raylib`):

- `main.c` is CMG_raylib's own source as of commit `bf1dc260d2`.
- `main.c.png` is the picture CMG_raylib made of the first 10 239 bytes of that file, at 5
  pixels per character (committed in `f93aaffc1d`).

The test `cmg::tests::the_picture_is_the_one_cmg_raylib_draws` checks that ACMS reads the same
picture from those bytes, pixel for pixel. The colour table is the same in every version of
CMG_raylib's `main.c` so far.
