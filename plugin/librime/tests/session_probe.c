// session_probe.c —— 直接驱动 librime 的 C API，验证 input_stat 插件是否真的生效。
//
// 与 weasel-rs 的 server 做法一致：设置 shared/user data 目录 → setup → initialize →
// 开会话 → 选方案 → 输入命令 → 打印候选。若统计 translator 正常，输入 /rtj 时应看到
// 报表或"※ 今天没有任何记录。"这样的候选。
//
// 编译（需要 librime 的导入库）：
//   clang -O1 -I <librime>/src session_probe.c <build>/src/librime.dll.a -o session_probe.exe
// 运行：在含 rime.dll 的目录下执行（rime.dll 与其依赖 DLL 都在那里）。

#include <stdio.h>
#include <string.h>

#include <rime_api.h>

/** 统计 needle 在 haystack 里出现的次数（用于判断一次提交里有几份报表）。 */
static int count_occurrences(const char* haystack, const char* needle) {
  if (haystack == NULL || needle == NULL || *needle == '\0') {
    return 0;
  }
  int count = 0;
  const size_t step = strlen(needle);
  for (const char* p = haystack; (p = strstr(p, needle)) != NULL; p += step) {
    ++count;
  }
  return count;
}

static void show(RimeApi* api, RimeSessionId session, const char* input) {
  printf("--- 输入 \"%s\"\n", input);
  if (!api->set_input(session, input)) {
    printf("    set_input 被拒绝\n");
    return;
  }
  RIME_STRUCT(RimeContext, context);
  if (!api->get_context(session, &context)) {
    printf("    get_context 失败\n");
    return;
  }
  printf("    候选数=%d  composition=\"%s\"\n", context.menu.num_candidates,
         context.composition.preedit ? context.composition.preedit : "");
  for (int index = 0; index < context.menu.num_candidates && index < 3; ++index) {
    const RimeCandidate* candidate = &context.menu.candidates[index];
    char text[200];
    snprintf(text, sizeof(text), "%s", candidate->text ? candidate->text : "");
    for (char* p = text; *p; ++p) {
      if (*p == '\n') { *p = '|'; }
      if (*p == '\r') { *p = ' '; }
    }
    printf("      [%d] text=%s\n", index, text);
  }
  api->free_context(&context);
}

int main(int argc, char** argv) {
  const char* shared = argc > 1 ? argv[1] : "rime-data";
  const char* user = argc > 2 ? argv[2] : ".";
  const char* schema = argc > 3 ? argv[3] : "sancang_simp";

  RimeApi* api = rime_get_api();
  if (!api) { printf("rime_get_api 返回空\n"); return 1; }

  RIME_STRUCT(RimeTraits, traits);
  traits.shared_data_dir = shared;
  traits.user_data_dir = user;
  traits.distribution_name = "weasel-rs";
  traits.distribution_code_name = "weasel-rs";
  traits.distribution_version = "0.1.0";
  traits.app_name = "rime.weasel-rs";
  api->setup(&traits);
  api->initialize(&traits);

  RimeSessionId session = api->create_session();
  if (!session) { printf("create_session 失败\n"); return 2; }
  if (api->select_schema) {
    printf("select_schema(%s) = %d\n", schema, api->select_schema(session, schema));
  }

  show(api, session, "/rtj");   // 日统计报表命令
  show(api, session, "/pf");    // 皮肤列表命令
  show(api, session, "nihao");  // 普通输入（应无统计候选）

  // 上屏一次（选中第 2 个候选），再查报表：验证"累积 → 报表"整条链路。
  if (api->select_candidate) {
    api->set_input(session, "nihao");
    printf("select_candidate(1) = %d\n", api->select_candidate(session, 1));
  }
  if (api->commit_composition) {
    printf("commit_composition = %d\n", api->commit_composition(session));
  }
  if (api->get_commit) {
    RIME_STRUCT(RimeCommit, commit);
    if (api->get_commit(session, &commit)) {
      printf("上屏文本: %s\n", commit.text ? commit.text : "(null)");
      api->free_commit(&commit);
    }
  }
  show(api, session, "/rtj");   // 应显示"上屏 1 次，输入 N 字"

  // 关键回归：把报表自身当普通候选上屏——它**不该**被计入统计。
  // 否则会出现"上屏 14 次却输入 1138 字"这种被报表长度灌水的结果。
  {
    // ── 逐键模拟真实输入：/ r t j 然后空格上屏 ──
    // 真实 IME 是逐键喂给 librime 的；`set_input` 一次性注入会掩盖分词差异
    // （RimeGetContext 只暴露最后一个 segment 的候选，而提交会拼接所有 segment）。
    printf("\n=== 逐键输入诊断（模拟真实打字）===\n");
    if (api->process_key && api->clear_composition) {
      api->clear_composition(session);
      const int keys[] = {'/', 'r', 't', 'j'};
      for (int i = 0; i < 4; ++i) {
        const int eaten = api->process_key(session, keys[i], 0);
        RIME_STRUCT(RimeContext, context);
        if (api->get_context(session, &context)) {
          printf("  按键 '%c' eaten=%d 组合=\"%s\" 候选数=%d\n", keys[i], eaten,
                 context.composition.preedit ? context.composition.preedit : "",
                 context.menu.num_candidates);
          api->free_context(&context);
        }
      }
      // 空格上屏（真实 IME 的做法），再看提交文本里有几份报表
      const int eaten = api->process_key(session, 32, 0);
      RIME_STRUCT(RimeCommit, commit);
      if (api->get_commit(session, &commit)) {
        const char* text = commit.text ? commit.text : "";
        printf("  空格后：eaten=%d 提交字节=%zu \"上屏\"出现=%d 次 \"极速\"出现=%d 次\n",
               eaten, strlen(text), count_occurrences(text, "上屏"),
               count_occurrences(text, "极速"));
        api->free_commit(&commit);
      } else {
        printf("  空格后 eaten=%d：没有提交文本\n", eaten);
      }
    }

    // ── 长款 /rtjj 诊断（用户报告"整份出现两次"的就是它）──
    printf("\n=== 长款 /rtjj 诊断 ===\n");
    if (api->set_input(session, "/rtjj")) {
      RIME_STRUCT(RimeContext, long_context);
      if (api->get_context(session, &long_context)) {
        printf("  /rtjj 候选数 = %d  (长款报表前两行)\n", long_context.menu.num_candidates);
        for (int index = 0; index < long_context.menu.num_candidates; ++index) {
          const char* text = long_context.menu.candidates[index].text
                                 ? long_context.menu.candidates[index].text
                                 : "";
          printf("    候选[%d] 字节=%zu 🚩出现=%d 次 脚本出现=%d 次\n", index, strlen(text),
                 count_occurrences(text, "🚩"), count_occurrences(text, "脚本："));
        }
        if (long_context.menu.num_candidates > 0 && long_context.menu.candidates[0].text) {
          char head[200];
          snprintf(head, sizeof(head), "%.160s", long_context.menu.candidates[0].text);
          printf("    报表开头: %s\n", head);
        }
        api->free_context(&long_context);
      }
    }
    if (api->select_candidate && api->commit_composition) {
      api->select_candidate(session, 0);
      api->commit_composition(session);
      RIME_STRUCT(RimeCommit, long_commit);
      if (api->get_commit(session, &long_commit)) {
        const char* text = long_commit.text ? long_commit.text : "";
        const size_t total = strlen(text);
        printf("  一次提交：字节=%zu 🚩出现=%d 次 脚本出现=%d 次\n", total,
               count_occurrences(text, "🚩"), count_occurrences(text, "脚本："));
        printf("  尾部 80 字节： ...%s\n", total > 80 ? text + total - 80 : text);
        api->free_commit(&long_commit);
      }
    }

    // ── 短款 /rtj 对照 ──
    printf("\n=== 短款 /rtj 对照 ===\n");
    if (api->set_input(session, "/rtj")) {
      RIME_STRUCT(RimeContext, context);
      if (api->get_context(session, &context)) {
        printf("  /rtj 候选数 = %d\n", context.menu.num_candidates);
        for (int index = 0; index < context.menu.num_candidates; ++index) {
          const char* text = context.menu.candidates[index].text
                                 ? context.menu.candidates[index].text
                                 : "";
          printf("    候选[%d] 字节=%zu \"上屏\"出现=%d 次  头=%.24s\n", index,
                 strlen(text), count_occurrences(text, "上屏"), text);
        }
        api->free_context(&context);
      }
    }
    if (api->select_candidate && api->commit_composition) {
      api->select_candidate(session, 0);
      api->commit_composition(session);
      RIME_STRUCT(RimeCommit, dup);
      if (api->get_commit(session, &dup)) {
        const char* text = dup.text ? dup.text : "";
        printf("  一次提交：字节=%zu \"上屏\"出现=%d 次 \"极速\"出现=%d 次\n", strlen(text),
               count_occurrences(text, "上屏"), count_occurrences(text, "极速"));
        api->free_commit(&dup);
      }
    }
    RIME_STRUCT(RimeContext, after);
    if (api->get_context(session, &after)) {
      printf("  提交后 composition=\"%s\" 候选数=%d（未清空会导致再按空格又提交一份）\n",
             after.composition.preedit ? after.composition.preedit : "",
             after.menu.num_candidates);
      api->free_context(&after);
    }
  }
  show(api, session, "/rtj");   // 数字必须与上一条 /rtj 完全一致

  api->destroy_session(session);
  api->finalize();
  printf("完成\n");
  return 0;
}
