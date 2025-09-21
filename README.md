
# mdbook-codetags

A preprocessor for [mdbook](https://rust-lang.github.io/mdBook/) implementing
code snippets generation as design by Robert Nystrom to produce
[Crafting Compilers](https://craftinginterpreters.com/).
The code is actually a port Nystrom's code to rust.

# Usage

Declare the preprocessor in your book configuration.
Either place `mdbook-codetags` in your PATH or specify the `command` option.
The `src-root` option indicate the source directory containing the sources to process.

Example configuration:

```
[preprocessor.codetags]
command = "mdbook-codetags"
src-root = "../craftinginterpreters/java"
```

# TODO

* clean code
* clean code again
* make the Java langage specifics configurable
