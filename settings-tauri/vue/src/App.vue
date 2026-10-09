<script setup lang="ts">
// 小狼毫 RS 设置（Tauri 界面）。
//
// 后端（src-tauri/src/lib.rs）复用 Slint 版同一套框架无关核心：加载配置、生成 schema
// 表单字段、校验、原子保存与冲突检测。这里只负责渲染与交互，所有状态都以一次性
// `settings_*` 命令的返回值（LoadReply）为准，组件内不复制配置状态。
import { computed, onMounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

interface FieldView {
  path: string[]
  key: string
  title: string
  description: string
  kind: 'color' | 'enum_number' | 'choice' | 'string' | 'json'
  values: unknown[]
  choices: string[]
  selected: number
  display_selected: number
  text: string
  boolean: boolean
  multiline: boolean
  inherited_text: string
  inherited_selected: number
  allow_system_color: boolean
  adaptive_color: boolean
  overridden: boolean
}

interface SectionView {
  title: string
  mount: string[]
  fields: FieldView[]
  effective: unknown
}

interface AppOptionView {
  app: string
  ascii_mode: boolean | null
  inline_preedit: boolean | null
  overridden: string[]
}

interface PathsView {
  config: string
  config_exists: boolean
  executable_directory: string
  user_data: string
  logs: string
}

interface LoadReply {
  sections: SectionView[]
  notices: string[]
  paths: PathsView
  dirty: boolean
  app_options: AppOptionView[]
}

const data = ref<LoadReply | null>(null)
const active = ref(0)
const busy = ref(false)
const error = ref('')
const status = ref('')
const newApp = ref('')

const section = computed<SectionView | null>(() => data.value?.sections[active.value] ?? null)

function fail(e: unknown): void {
  error.value = typeof e === 'string' ? e : e instanceof Error ? e.message : String(e)
  status.value = ''
}

async function run(command: string, args?: Record<string, unknown>): Promise<void> {
  busy.value = true
  error.value = ''
  try {
    data.value = await invoke<LoadReply>(command, args)
  } catch (e) {
    fail(e)
  } finally {
    busy.value = false
  }
}

function load(): Promise<void> {
  return run('settings_load')
}

/** 应用一个字段：`selected` 为 0 表示恢复继承，`text` 供字符串/JSON/自定义数值使用。 */
async function applyField(
  index: number,
  field: FieldView,
  selected: number,
  text: string
): Promise<void> {
  status.value = ''
  await run('settings_apply', {
    section: active.value,
    field: index,
    selected,
    text
  })
  if (!error.value) {
    status.value = `已修改 ${field.title}（未保存）`
  }
}

/** 输入框改动：字符串/颜色/JSON 一旦输入即视为覆盖。 */
function editText(index: number, field: FieldView, event: Event): void {
  const target = event.target as HTMLInputElement | HTMLTextAreaElement
  const selected = field.selected === 0 ? 1 : field.selected
  void applyField(index, field, selected, target.value)
}

function toggleOverride(index: number, field: FieldView, event: Event): void {
  const target = event.target as HTMLInputElement
  const text = target.checked ? (field.overridden ? field.text : field.inherited_text) : field.text
  void applyField(index, field, target.checked ? field.selected || 1 : 0, text)
}

function selectChoice(index: number, field: FieldView, event: Event): void {
  const target = event.target as HTMLSelectElement
  void applyField(index, field, Number(target.value), field.text)
}

/** 自定义数值控件（enum_number 的最后一项）只在选中它时可编辑。 */
function isCustomNumber(field: FieldView): boolean {
  return field.kind === 'enum_number' && field.selected === field.values.length + 1
}

function setApp(app: string, key: 'ascii_mode' | 'inline_preedit', value: string): void {
  const parsed = value === 'inherit' ? null : value === 'true'
  void run('settings_app_set', { app, key, value: parsed })
}

function addApp(): void {
  if (!newApp.value.trim()) return
  void run('settings_app_add', { app: newApp.value.trim() }).then(() => {
    if (!error.value) newApp.value = ''
  })
}

function pretty(value: unknown): string {
  return JSON.stringify(value, null, 2)
}

onMounted(load)
</script>

<template>
  <main class="page">
    <header>
      <h1>小狼毫 RS 设置</h1>
      <div class="toolbar">
        <button :disabled="busy" @click="load">重新加载</button>
        <button :disabled="busy" @click="run('settings_discard')">丢弃改动</button>
        <button :disabled="busy" @click="run('settings_reset')">恢复默认（清空覆盖）</button>
        <button class="primary" :disabled="busy || !data?.dirty" @click="run('settings_save')">
          保存{{ data?.dirty ? ' *' : '' }}
        </button>
        <span v-if="busy" class="hint">处理中…</span>
      </div>
    </header>

    <p v-if="error" class="banner error">{{ error }}</p>
    <p v-else-if="status" class="banner ok">{{ status }}</p>

    <p v-if="data" class="hint">
      配置文件：<code>{{ data.paths.config }}</code>
      （{{ data.paths.config_exists ? '已存在' : '尚未创建，保存后生成' }}）
    </p>
    <p v-for="notice in data?.notices ?? []" :key="notice" class="banner warn">{{ notice }}</p>

    <nav v-if="data" class="tabs">
      <button
        v-for="(item, index) in data.sections"
        :key="item.title"
        :class="{ tab: true, active: index === active }"
        @click="active = index"
      >
        {{ item.title }}
      </button>
    </nav>

    <section v-if="section" class="panel">
      <div v-if="section.fields.length === 0" class="hint">该分区没有可编辑字段。</div>

      <div v-for="(field, index) in section.fields" :key="field.key" class="field">
        <div class="field-head">
          <label class="field-title">{{ field.title }}</label>
          <span class="key">{{ field.key }}</span>
          <span v-if="field.overridden" class="badge">已覆盖</span>
        </div>
        <p v-if="field.description" class="desc">{{ field.description }}</p>

        <div class="row">
          <label v-if="field.kind !== 'choice' && field.kind !== 'enum_number'" class="check">
            <input
              type="checkbox"
              :checked="field.overridden"
              :disabled="busy"
              @change="toggleOverride(index, field, $event)"
            />
            覆盖（默认 {{ field.inherited_text }}）
          </label>

          <template v-if="field.kind === 'choice' || field.kind === 'enum_number'">
            <select :value="field.selected" :disabled="busy" @change="selectChoice(index, field, $event)">
              <option v-for="(choice, choiceIndex) in field.choices" :key="choiceIndex" :value="choiceIndex">
                {{ choice }}
              </option>
            </select>
            <input
              v-if="isCustomNumber(field)"
              class="text"
              type="number"
              :value="field.text"
              :disabled="busy"
              @change="editText(index, field, $event)"
            />
          </template>

          <template v-else-if="field.kind === 'json'">
            <textarea
              class="json"
              :rows="field.multiline ? 6 : 2"
              :value="field.text"
              :disabled="busy"
              @change="editText(index, field, $event)"
            />
          </template>

          <template v-else>
            <input
              class="text"
              type="text"
              :value="field.text"
              :disabled="busy"
              :placeholder="field.inherited_text"
              @change="editText(index, field, $event)"
            />
            <button
              v-if="field.kind === 'color' && field.allow_system_color"
              :disabled="busy"
              @click="applyField(index, field, field.selected || 1, 'system')"
            >
              跟随系统
            </button>
          </template>
        </div>
      </div>

      <details class="advanced">
        <summary>该分区生效值（JSON，只读）</summary>
        <pre>{{ pretty(section.effective) }}</pre>
      </details>
    </section>

    <section v-if="data" class="panel">
      <h2>按应用覆盖</h2>
      <p class="hint">为特定程序覆盖输入选项；"继承"表示不写入覆盖、沿用安装配置。</p>
      <table>
        <thead>
          <tr><th>程序</th><th>中英状态</th><th>外置编码</th><th></th></tr>
        </thead>
        <tbody>
          <tr v-for="option in data.app_options" :key="option.app">
            <td><code>{{ option.app }}</code></td>
            <td>
              <select
                :value="option.overridden.includes('ascii_mode') ? String(option.ascii_mode) : 'inherit'"
                :disabled="busy"
                @change="setApp(option.app, 'ascii_mode', ($event.target as HTMLSelectElement).value)"
              >
                <option value="inherit">继承</option>
                <option value="true">开</option>
                <option value="false">关</option>
              </select>
            </td>
            <td>
              <select
                :value="option.overridden.includes('inline_preedit') ? String(option.inline_preedit) : 'inherit'"
                :disabled="busy"
                @change="setApp(option.app, 'inline_preedit', ($event.target as HTMLSelectElement).value)"
              >
                <option value="inherit">继承</option>
                <option value="true">开</option>
                <option value="false">关</option>
              </select>
            </td>
            <td>
              <button :disabled="busy" @click="run('settings_app_remove', { app: option.app })">删除</button>
            </td>
          </tr>
        </tbody>
      </table>
      <div class="row">
        <input v-model="newApp" class="text" placeholder="notepad.exe" :disabled="busy" />
        <button :disabled="busy" @click="addApp">新增程序</button>
      </div>
    </section>

    <footer v-if="data" class="hint">
      安装目录：<code>{{ data.paths.executable_directory }}</code><br />
      用户数据：<code>{{ data.paths.user_data }}</code><br />
      日志：<code>{{ data.paths.logs }}</code>
    </footer>
  </main>
</template>

<style scoped>
.page { padding: 1.25rem 1.5rem 2rem; font-family: system-ui, "Segoe UI", sans-serif; max-width: 960px; margin: 0 auto; color: #1f2328; }
h1 { font-size: 1.25rem; margin: 0 0 0.75rem; }
h2 { font-size: 1rem; margin: 0 0 0.5rem; }
.toolbar { display: flex; gap: 0.5rem; flex-wrap: wrap; align-items: center; }
button { padding: 0.35rem 0.8rem; border: 1px solid #d0d7de; border-radius: 6px; background: #f6f8fa; cursor: pointer; font-size: 0.875rem; }
button:hover:not(:disabled) { background: #eaeef2; }
button:disabled { opacity: 0.5; cursor: default; }
button.primary { background: #1f6feb; border-color: #1f6feb; color: #fff; }
button.primary:hover:not(:disabled) { background: #1a5fd0; }
.tabs { display: flex; gap: 0.35rem; flex-wrap: wrap; margin: 1rem 0 0.5rem; }
.tab.active { background: #1f6feb; border-color: #1f6feb; color: #fff; }
.panel { border: 1px solid #d0d7de; border-radius: 8px; padding: 1rem; margin-bottom: 1rem; background: #fff; }
.field { padding: 0.6rem 0; border-bottom: 1px dashed #e5e9ef; }
.field:last-of-type { border-bottom: none; }
.field-head { display: flex; align-items: center; gap: 0.5rem; }
.field-title { font-weight: 600; }
.key { color: #6e7781; font-size: 0.75rem; }
.badge { background: #ddf4ff; color: #0969da; border-radius: 999px; padding: 0 0.5rem; font-size: 0.75rem; }
.desc { color: #57606a; font-size: 0.8125rem; margin: 0.2rem 0 0.4rem; }
.row { display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; }
.check { display: flex; align-items: center; gap: 0.35rem; font-size: 0.875rem; }
select, input.text, textarea.json { padding: 0.3rem 0.5rem; border: 1px solid #d0d7de; border-radius: 6px; font-size: 0.875rem; font-family: inherit; }
input.text { min-width: 14rem; }
textarea.json { width: 100%; font-family: ui-monospace, Consolas, monospace; }
.advanced { margin-top: 1rem; }
pre { background: #f6f8fa; padding: 0.75rem; border-radius: 6px; overflow: auto; font-size: 0.8125rem; }
table { width: 100%; border-collapse: collapse; font-size: 0.875rem; }
th, td { text-align: left; padding: 0.35rem 0.5rem; border-bottom: 1px solid #eaeef2; }
code { background: #f6f8fa; padding: 0 0.25rem; border-radius: 3px; }
.hint { color: #57606a; font-size: 0.8125rem; }
.banner { border-radius: 6px; padding: 0.5rem 0.75rem; font-size: 0.875rem; }
.banner.error { background: #ffebe9; color: #b4222b; }
.banner.ok { background: #dafbe1; color: #0f5323; }
.banner.warn { background: #fff8c5; color: #6b5900; }
footer { margin-top: 1rem; line-height: 1.6; }
</style>
