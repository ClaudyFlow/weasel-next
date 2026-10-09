// table_dump.c —— 用 librime 自己的词典（prism + table）把"码 → 词条"转储出来。
//
// 不去逆 StringTable 的压缩格式（它是排序 + 前缀共享存储，逐字节扫只能得到碎片），
// 而是驱动引擎逐码查询：set_input(code) → get_context() → 打印候选。
// 候选就是 table 里该码命中的词条（经方案翻译器排序后的结果），权重顺序即候选顺序。
//
// 编译：
//   clang -O1 -I <librime>/src table_dump.c <build>/src/librime.dll.a -o table_dump.exe
// 运行（在含 rime.dll 的目录下）：
//   table_dump.exe <rime-data> <user-data> <schema-id> <输出文件>

#include <stdio.h>
#include <string.h>

#include <rime_api.h>

static int g_with_candidates = 0;
static int g_queries = 0;
static long g_total_candidates = 0;

static void query(RimeApi* api, RimeSessionId session, const char* code, FILE* out,
                  int* best_code_count) {
  ++g_queries;
  if (!api->set_input(session, code)) {
    return;
  }
  RIME_STRUCT(RimeContext, context);
  if (!api->get_context(session, &context)) {
    return;
  }
  const int count = context.menu.num_candidates;
  if (count > 0) {
    ++g_with_candidates;
    g_total_candidates += count;
    if (count > *best_code_count) {
      *best_code_count = count;
    }
    fprintf(out, "%s\t%d\t", code, count);
    for (int i = 0; i < count && i < 8; ++i) {
      const char* text = context.menu.candidates[i].text ? context.menu.candidates[i].text : "";
      fprintf(out, "%s%s", i ? " | " : "", text);
    }
    fprintf(out, "\n");
  }
  api->free_context(&context);
}

int main(int argc, char** argv) {
  if (argc < 5) {
    printf("用法: table_dump.exe <rime-data> <user-data> <schema-id> <输出文件>\n");
    return 2;
  }
  const char* shared = argv[1];
  const char* user = argv[2];
  const char* schema = argv[3];
  FILE* out = fopen(argv[4], "wb");
  if (out == NULL) {
    printf("无法写出 %s\n", argv[4]);
    return 3;
  }

  RimeApi* api = rime_get_api();
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
  if (!session) {
    printf("create_session 失败\n");
    return 4;
  }
  if (api->select_schema) {
    api->select_schema(session, schema);
  }

  int best = 0;
  fprintf(out, "# 码\t候选数\t候选（最多 8 个）\n");
  char code[8];

  // 1 字母
  for (int a = 'a'; a <= 'z'; ++a) {
    code[0] = (char)a;
    code[1] = 0;
    query(api, session, code, out, &best);
  }
  // 2 字母
  for (int a = 'a'; a <= 'z'; ++a) {
    for (int b = 'a'; b <= 'z'; ++b) {
      code[0] = (char)a;
      code[1] = (char)b;
      code[2] = 0;
      query(api, session, code, out, &best);
    }
  }
  // 3 字母：抽样（每 17 个取 1），覆盖整个空间
  int seen = 0;
  for (int a = 'a'; a <= 'z'; ++a) {
    for (int b = 'a'; b <= 'z'; ++b) {
      for (int c = 'a'; c <= 'z'; ++c) {
        if (seen++ % 17) {
          continue;
        }
        code[0] = (char)a;
        code[1] = (char)b;
        code[2] = (char)c;
        code[3] = 0;
        query(api, session, code, out, &best);
      }
    }
  }

  fclose(out);
  api->destroy_session(session);
  api->finalize();
  printf("查询 %d 个码，其中 %d 个有候选（%.1f%%），候选总数 %ld，单码最多 %d 个\n", g_queries,
         g_with_candidates, 100.0 * g_with_candidates / (g_queries ? g_queries : 1),
         g_total_candidates, best);
  return 0;
}
