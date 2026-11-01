/* llvm-mingw 的 i686 目标里没有 gcc 版本的 libgcc_eh.a：
 * i686 只带 libunwind.a，而 MinGW 的 crtbegin.o 仍会调用 gcc 的
 * __register_frame_info / __deregister_frame_info（DWARF2 帧注册）。
 *
 * 这里给两个空实现，只为让 32 位 TIP 能链上：
 *   * 我们不在 DLL 里做跨模块 C++ 展开，注册表没有消费者；
 *   * Rust 侧真要 panic 也会在宿主进程里直接失败，空实现不会让情况更糟。
 *
 * x86_64 不需要这份垫片（那边的 libgcc*.a 是 libunwind 的副本，能解析这些符号）。
 */
void __register_frame_info(const void *begin, void *object) {
    (void)begin;
    (void)object;
}

void __deregister_frame_info(const void *begin) {
    (void)begin;
}
