# unit_test_example_logs provenance

These fixtures are excerpted from public real-world logs and documentation output.
They are intentionally trimmed to the minimum lines needed by tests.

## npm

- `npm_success.log`
  - Source: https://stackoverflow.com/questions/67630511/package-json-up-to-date-audited-1-package-in-1s-found-0-vulnerabilities
  - Excerpt: `up to date, audited 1 package in 1s`, `found 0 vulnerabilities`
- `npm_warning.log`
  - Source: https://stackoverflow.com/questions/58989617/npm-warn-deprecated-what-does-it-mean
  - Excerpt: `npm WARN deprecated ...`
- `npm_error.log`
  - Source: https://stackoverflow.com/questions/75453409/npm-install-giving-error-on-node-gyp-rebuild
  - Excerpt: `npm ERR! code 1`, `npm ERR! command failed`, `npm ERR! ...`

## Maven

- `maven_success.log`
  - Source: https://stackoverflow.com/questions/26247488/maven-install-issue-build-successful-i-dont-understand-how-to-read-the-outp
  - Excerpt: `[INFO] Scanning for projects...`, `[INFO] BUILD SUCCESS`
- `maven_warning.log`
  - Source: https://stackoverflow.com/questions/4394450/warning-when-build-maven
  - Excerpt: `[WARNING] Failed to initialize environment variable resolver...`
- `maven_error.log`
  - Source: https://stackoverflow.com/questions/8175525/maven-error-build-failure
  - Excerpt: `[INFO] BUILD FAILURE`, `[ERROR] Failed to execute goal ...`

## C/C++

- `cpp_warning.log`
  - Source: https://clang.llvm.org/diagnostics.html
  - Excerpt: `format-strings.c:91:13: warning: ...`
- `cpp_error.log`
  - Sources:
    - https://clang.llvm.org/diagnostics.html
    - https://stackoverflow.com/questions/60019491/undefined-reference-to-error-while-linking-files-using-g-in-linux
  - Excerpt: `t.c:7:39: error: ...`, `undefined reference to`, `collect2: error: ...`

## License notes

- Stack Overflow content is licensed under CC BY-SA 4.0.
- Clang documentation content is cited from official LLVM/Clang docs.
