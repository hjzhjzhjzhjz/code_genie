// =========================================================================
// 🧪 librime 逐行解码器
// =========================================================================
//
// 读 stdin 一行一个按键序列，走 librime 的 simulate_key_sequence，
// 把 commit 结果一行一个写回 stdout（没提交就写空行）。
//
// 与 librime 自带的 rime_api_console 相比：
//   1. 共享/用户目录由 --shared / --user 显式给出，不猜 ~/.config；
//   2. 输出只有 commit 文本，不夹 menu / status，方便逐行对齐；
//   3. 空格没提交时兜底选第 1 候选，并清空残留组合，避免串味。
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
  int verbose = 0;
  for (int i = 1; i < argc; ++i) {
    if (!std::strcmp(argv[i], "--shared") && i + 1 < argc) {
      shared = argv[++i];
    } else if (!std::strcmp(argv[i], "--user") && i + 1 < argc) {
      user = argv[++i];
    } else if (!std::strcmp(argv[i], "--schema") && i + 1 < argc) {
      schema_id = argv[++i];
    } else if (!std::strcmp(argv[i], "--verbose")) {
      verbose = 1;
    }
  }
  if (!shared || !user) {
    std::fprintf(stderr,
                 "用法: rime_eval --shared <共享目录> --user <用户目录> "
                 "[--schema <方案 id>] [--verbose]\n");
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
  traits.min_log_level = verbose ? 2 : 3;

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
  int 已诊断 = 0;
  while (std::getline(std::cin, line)) {
    while (!line.empty() && (line.back() == '\r' || line.back() == '\n')) {
      line.pop_back();
    }
    if (line.empty()) {
      line = " ";
    }

    RIME_STRUCT(RimeCommit, commit);
    bool 有提交 = false;

    if (rime->simulate_key_sequence(session, line.c_str()) &&
        rime->get_commit(session, &commit)) {
      有提交 = true;
    }

    if (!有提交) {
      // 空格没提交：看一眼候选，能选就选第 1 个
      RIME_STRUCT(RimeContext, ctx);
      if (rime->get_context(session, &ctx)) {
        if (已诊断 < 3) {
          std::fprintf(stderr,
                       "[诊断] 组合长度=%d 候选=%d 首候选=%s 预显=%s\n",
                       (int)ctx.composition.length, ctx.menu.num_candidates,
                       ctx.menu.num_candidates > 0 ? ctx.menu.candidates[0].text
                                                   : "(无)",
                       ctx.composition.preedit ? ctx.composition.preedit : "");
          ++已诊断;
        }
        int 候选数 = ctx.menu.num_candidates;
        rime->free_context(&ctx);
        if (候选数 > 0 &&
            rime->select_candidate_on_current_page(session, 0) &&
            rime->get_commit(session, &commit)) {
          有提交 = true;
        }
      }
    }

    if (有提交) {
      std::printf("%s\n", commit.text ? commit.text : "");
      rime->free_commit(&commit);
    } else {
      // 残留组合会污染下一行
      rime->clear_composition(session);
      std::printf("\n");
    }
    std::fflush(stdout);
  }

  rime->destroy_session(session);
  rime->finalize();
  return 0;
}
