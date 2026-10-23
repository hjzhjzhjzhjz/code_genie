// =========================================================================
// 🧪 librime 逐行解码器
// =========================================================================
//
// 读 stdin 一行一个按键序列，走 librime 的 simulate_key_sequence，
// 把 commit 结果一行一个写回 stdout（没提交就写空行）。
//
// 与 librime 自带的 rime_api_console 相比只有两点不同：
//   1. 共享/用户目录由 --shared / --user 显式给出，不猜 ~/.config；
//   2. 输出只有 commit 文本，不夹 menu / status，方便逐行对齐。
//
// 编译：g++ -O2 tools/rime_eval.cc -o rime_eval $(pkg-config --cflags --libs rime)
// =========================================================================

#include <rime_api.h>

#include <cstdio>
#include <cstring>
#include <string>
#include <iostream>

static std::string schema_id = "xingyun_test";

int main(int argc, char** argv) {
  const char* shared = nullptr;
  const char* user = nullptr;
  for (int i = 1; i < argc; ++i) {
    if (!std::strcmp(argv[i], "--shared") && i + 1 < argc) {
      shared = argv[++i];
    } else if (!std::strcmp(argv[i], "--user") && i + 1 < argc) {
      user = argv[++i];
    } else if (!std::strcmp(argv[i], "--schema") && i + 1 < argc) {
      schema_id = argv[++i];
    }
  }
  if (!shared || !user) {
    std::fprintf(stderr,
                 "用法: rime_eval --shared <共享目录> --user <用户目录> "
                 "[--schema <方案 id>]\n");
    return 2;
  }

  RimeApi* rime = rime_get_api();
  if (!rime) {
    std::fprintf(stderr, "拿不到 rime api\n");
    return 1;
  }

  RIME_STRUCT(RimeTraits, traits);
  traits.shared_data_dir = shared;
  traits.user_data_dir = user;
  traits.app_name = "rime.eval";
  traits.distribution_name = "Rime";
  traits.distribution_code_name = "rime-eval";
  traits.distribution_version = "1.0";
  traits.min_log_level = 3;  // 只看 error

  rime->setup(&traits);
  rime->initialize(nullptr);
  if (rime->start_maintenance(True)) {
    rime->join_maintenance_thread();
  }

  RimeSessionId session = rime->create_session();
  if (!session) {
    std::fprintf(stderr, "建不了 session\n");
    return 1;
  }

  if (RIME_API_AVAILABLE(rime, select_schema)) {
    if (!rime->select_schema(session, schema_id.c_str())) {
      std::fprintf(stderr, "选不了方案 %s\n", schema_id.c_str());
      return 1;
    }
  }

  std::fprintf(stderr, "ready schema=%s shared=%s user=%s\n", schema_id.c_str(),
               shared, user);

  std::string line;
  while (std::getline(std::cin, line)) {
    while (!line.empty() && (line.back() == '\r' || line.back() == '\n')) {
      line.pop_back();
    }
    if (line.empty()) {
      line = " ";
    }
    Bool ok = rime->simulate_key_sequence(session, line.c_str());
    if (!ok) {
      std::fprintf(stderr, "按键序列失败: %s\n", line.c_str());
      std::printf("\n");
      std::fflush(stdout);
      continue;
    }
    RIME_STRUCT(RimeCommit, commit);
    if (rime->get_commit(session, &commit)) {
      std::printf("%s\n", commit.text ? commit.text : "");
      rime->free_commit(&commit);
    } else {
      std::printf("\n");
    }
    std::fflush(stdout);
  }

  rime->destroy_session(session);
  rime->finalize();
  return 0;
}
