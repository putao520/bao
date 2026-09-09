/* Target-detection TU (issue #10 / REQ-DEPLOY-1 p3.5).
 *
 * Compiling this file successfully proves the compiler is targeting musl,
 * not glibc: musl never defines __GLIBC__. Used as the acceptance criterion
 * for clang-built artifacts whose .comment producer cannot distinguish the
 * target (see scripts/clang-musl.sh). Compilation only — never linked.
 */
#ifdef __GLIBC__
#error "glibc-target detected: this compiler invocation is NOT musl"
#endif

int bao_musl_target_check(void) { return 0; }
