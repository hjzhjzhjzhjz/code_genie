#!/usr/bin/env tsx
/**
 * 给方案档案补上「連續文本當量」这一段。
 *
 * 口径与 recompute-schemes.mts 一致：
 *   清洗連續文本(literature.txt) → 蒙特卡洛連續文本當量(窗口 100 / 样本 20000)
 *   → 壓縮連續文本當量結果（存格点计数，几 KB）
 *
 * 用法（在 yuhao-assess 仓库根目录）：
 *   pnpm tsx -r tsconfig-paths/register scripts/fill-continuous.mts \
 *     --profile 方案档案/xxx.json --table 全码表.txt [--samples 20000] [--window 100]
 */
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const APP_DIR = path.resolve(__dirname, '..')
const PUBLIC_DIR = path.join(APP_DIR, 'public')

// 服务层用 fetch('/settings/...') 读数据，这里改读本地 public/
const 原生fetch = globalThis.fetch
globalThis.fetch = (async (資源: unknown, 選項?: unknown) => {
  const url = String(資源)
  if (url.startsWith('/')) {
    const 文件路徑 = path.join(PUBLIC_DIR, url)
    if (!fs.existsSync(文件路徑)) return new Response('', { status: 404 })
    return new Response(fs.readFileSync(文件路徑))
  }
  return 原生fetch(資源 as string, 選項 as RequestInit)
}) as typeof fetch

const { 碼表處理服務實例 } = await import('../src/services/codeTableService.ts')
const { 清洗連續文本, 蒙特卡洛連續文本當量 } =
  await import('../src/services/continuousEquivalentService.ts')
const { 壓縮連續文本當量結果 } = await import('../src/atoms/continuousEquivalent.ts')
const { 默認選重鍵表 } = await import('../src/types/scheme.ts')

function 取參(名: string, 缺省?: string): string | undefined {
  const i = process.argv.indexOf(名)
  return i >= 0 ? process.argv[i + 1] : 缺省
}

const 檔案路徑 = 取參('--profile')
const 碼表路徑 = 取參('--table')
if (!檔案路徑 || !碼表路徑) {
  console.error('用法：--profile <方案档案.json> --table <码表.txt> [--samples N] [--window N]')
  process.exit(1)
}
const 窗口長度 = Number(取參('--window', '100'))
const 樣本數 = Number(取參('--samples', '20000'))

const 读Json = (路径: string) => JSON.parse(fs.readFileSync(路径, 'utf8').replace(/^\uFEFF/, ''))

const 配置 = 读Json(檔案路徑)
const 分隔符 = (配置.碼表元數據?.分隔符 ?? '製表符') as '空格' | '製表符' | '逗號' | '分號'
const 第一列類型 = (配置.碼表元數據?.第一列類型 ?? '字符') as '字符' | '編碼'
const 最大碼長 = Number(配置.方案參數?.最大碼長 ?? 4)

const 碼表文本 = fs.readFileSync(碼表路徑, 'utf8')
const { rawCodeTable } = await 碼表處理服務實例.解析原始碼表文本(碼表文本, 分隔符, 第一列類型)
const 處理結果 = await 碼表處理服務實例.處理原始碼表(rawCodeTable, { 最大碼長 })

const 當量表 = (
  读Json(path.join(PUBLIC_DIR, 'settings/equivTable.json')) as {
    data: Record<string, number>
  }
).data

const 語料 = 清洗連續文本(
  fs.readFileSync(path.join(PUBLIC_DIR, 'texts/literature.txt'), 'utf8'),
)
console.log(`語料 ${語料.length} 字　窗口 ${窗口長度}　樣本 ${樣本數}`)

const 選重鍵表 = 配置.方案參數?.選重鍵表 ?? 默認選重鍵表

const 統計 = {
  全碼加選重: 蒙特卡洛連續文本當量(語料, 處理結果.全碼加選重鍵表, 當量表, {
    窗口長度,
    樣本數,
    選重鍵表,
  }),
  全部簡碼加選重: 蒙特卡洛連續文本當量(語料, 處理結果.簡碼加選重鍵表, 當量表, {
    窗口長度,
    樣本數,
    選重鍵表,
  }),
}

配置.測評結果 = {
  ...(配置.測評結果 ?? {}),
  連續文本當量: 壓縮連續文本當量結果({
    統計,
    樣本數,
    窗口長度,
    更新時間: new Date().toISOString(),
  }),
}

fs.writeFileSync(檔案路徑, JSON.stringify(配置, null, 2) + '\n', 'utf8')

const 全 = 統計.全碼加選重
const 簡 = 統計.全部簡碼加選重
console.log(
  `全碼 μ ${全.平均數.toFixed(4)}　中位 ${全.中位數.toFixed(4)}　P90 ${全.九零分位數.toFixed(4)}`,
)
console.log(
  `全簡 μ ${簡.平均數.toFixed(4)}　中位 ${簡.中位數.toFixed(4)}　P90 ${簡.九零分位數.toFixed(4)}`,
)
console.log(`已寫入 ${檔案路徑}`)
