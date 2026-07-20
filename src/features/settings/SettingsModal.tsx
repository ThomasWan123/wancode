/* v0.13 拆分：设置弹窗（步 A：纯 JSX 搬移，全部依赖经 props 透传；
   步 B 将把仅设置页使用的状态与处理器迁入本文件）。 */
import { invoke } from "@tauri-apps/api/core";
import { saveLang, type Lang } from "../../i18n";
import { IconX, IconPencil } from "../../icons";

const MODEL_PRESETS: Record<string, { name: string; model: string; base_url: string }> = {
  DeepSeek: { name: "DeepSeek V3", model: "deepseek-chat", base_url: "https://api.deepseek.com/v1" },
  "智谱 GLM": { name: "智谱 GLM-4-Flash", model: "glm-4-flash", base_url: "https://open.bigmodel.cn/api/paas/v4" },
  OpenAI: { name: "GPT-4o", model: "gpt-4o", base_url: "https://api.openai.com/v1" },
  Ollama: { name: "Ollama (本地)", model: "qwen2.5-coder", base_url: "http://localhost:11434/v1" },
};

export function SettingsModal(props: Record<string, any>) {
  const { showSettings, hookForm, lang, mcpForm, mcpList, mcpLive, migrateMsg, modelForm, modelList, modelTestMsg, openSkillEditor, quickBusy, quickKey, quickPreset, quickResult, refreshMcpConfig, refreshMcpLive, refreshModels, refreshSessions, refreshSkills, runUpdate, saveHooks, saveModel, setError, setHookForm, setLang, setMcpForm, setMigrateMsg, setModelForm, setQuickBusy, setQuickKey, setQuickPreset, setQuickResult, setSettingsTab, setShowSettings, setSkillForm, setSkills, setTheme, settingsTab, skillForm, skills, testModel, theme, updateMsg, version, workspace, hooks, t } = props;
  return (
    <>
      {showSettings && (
        <div className="modal-mask" onClick={() => setShowSettings(false)}>
          <div className="modal settings-modal" onClick={(e) => e.stopPropagation()}>
            <nav className="settings-nav">
              <div className="settings-nav-title">{t.settingsTitle}</div>
              {([
                ["general", t.navGeneral],
                ["models", t.navModels],
                ["mcp", t.navMcp],
                ["skills", t.navSkills],
                ["hooks", t.navHooks],
                ["about", t.navAbout],
              ] as const).map(([id, label]) => (
                <button
                  key={id}
                  className={`settings-nav-item ${settingsTab === id ? "active" : ""}`}
                  onClick={() => setSettingsTab(id)}
                >
                  {label}
                </button>
              ))}
            </nav>
            <div className="settings-content">
            {settingsTab === "general" && (
            <div className="modal-section">
              <div className="modal-label">{t.language}</div>
              <select
                value={lang}
                onChange={(e) => {
                  const l = e.currentTarget.value as Lang;
                  setLang(l);
                  saveLang(l);
                }}
              >
                <option value="zh">中文</option>
                <option value="en">English</option>
              </select>
              <div className="modal-label" style={{ marginTop: 16 }}>{t.themeLabel}</div>
              <select value={theme} onChange={(e) => setTheme(e.currentTarget.value as "dark" | "light")}>
                <option value="dark">{t.themeDark}</option>
                <option value="light">{t.themeLight}</option>
              </select>
            </div>
            )}
            {settingsTab === "models" && (
            <div className="modal-section">
              {/* 一键配置：小白路径。选卡片 → 贴 Key → 完事。
                  Coding Plan 与开放平台是不同端点、Key 不通用——这是最常见
                  的配错点，所以拆成两张卡而不是一个开关。 */}
              <div className="modal-label">{t.quickSetupTitle}</div>
              <div className="preset-cards">
                {([
                  ["glm-coding", "GLM Coding Plan", t.presetGlmCoding],
                  ["glm-open", "智谱开放平台", t.presetGlmOpen],
                  ["deepseek", "DeepSeek", t.presetDeepseek],
                ] as const).map(([id, label, hint]) => (
                  <button
                    key={id}
                    className={`preset-card ${quickPreset === id ? "active" : ""}`}
                    onClick={() => setQuickPreset(quickPreset === id ? "" : id)}
                  >
                    <b>{label}</b>
                    <span>{hint}</span>
                  </button>
                ))}
              </div>
              {quickPreset && (
                <div className="quick-key-row">
                  <input
                    type="password"
                    placeholder={t.quickKeyPlaceholder}
                    value={quickKey}
                    onChange={(e) => setQuickKey(e.target.value)}
                  />
                  <button
                    disabled={!quickKey.trim() || quickBusy}
                    onClick={async () => {
                      setQuickBusy(true);
                      setQuickResult("");
                      try {
                        const r = await invoke<any>("provider_quick_setup", {
                          preset: quickPreset,
                          apiKey: quickKey,
                        });
                        const names = (r.models ?? []).map((m: any) => m.name).join("、");
                        setQuickResult(
                          `✅ ${t.quickDone}${names}${r.mcpSeeded ? ` · ${t.quickMcpSeeded}` : ""}`,
                        );
                        setQuickKey("");
                        refreshModels();
                      } catch (e) {
                        setQuickResult(`❌ ${String(e)}`);
                      } finally {
                        setQuickBusy(false);
                      }
                    }}
                  >
                    {quickBusy ? t.quickTesting : t.quickGo}
                  </button>
                </div>
              )}
              {quickResult && <div className="quick-result">{quickResult}</div>}

              <div className="modal-label">{t.modelsSection}</div>
              <div className="mcp-list">
                {modelList.length === 0 && <div className="sidebar-empty">{t.modelsEmpty}</div>}
                {modelList.map((m: any) => (
                  <div key={m.key} className="mcp-item">
                    <div className="mcp-info">
                      <b>
                        {m.name}{" "}
                        <span className={m.has_key ? "key-ok" : "key-warn"}>
                          {m.has_key ? t.modelKeyStored : t.modelKeyMissing}
                        </span>
                      </b>
                      <span className="mcp-detail">
                        {m.model} · {m.base_url}
                      </span>
                    </div>
                    <button
                      className="ghost small"
                      title={t.modelDelete}
                      onClick={async () => {
                        await invoke("model_remove", { key: m.key }).catch((e) => setError(String(e)));
                        refreshModels();
                      }}
                    >
                      <IconX size={13} />
                    </button>
                  </div>
                ))}
              </div>
              <div className="model-presets">
                <span className="preset-label">{t.modelPreset}:</span>
                {Object.entries(MODEL_PRESETS).map(([label, p]) => (
                  <button
                    key={label}
                    className="chip preset-chip"
                    onClick={() =>
                      setModelForm({
                        key: modelForm.key || label.toLowerCase().replace(/\s+/g, "-"),
                        name: p.name,
                        model: p.model,
                        base_url: p.base_url,
                        api_key: modelForm.api_key,
                      })
                    }
                  >
                    {label}
                  </button>
                ))}
              </div>
              <div className="mcp-form">
                <input
                  placeholder={t.modelKeyField}
                  value={modelForm.key}
                  onChange={(e) => setModelForm({ ...modelForm, key: e.currentTarget.value })}
                />
                <input
                  placeholder={t.modelDisplayName}
                  value={modelForm.name}
                  onChange={(e) => setModelForm({ ...modelForm, name: e.currentTarget.value })}
                />
                <input
                  placeholder={t.modelIdField}
                  value={modelForm.model}
                  onChange={(e) => setModelForm({ ...modelForm, model: e.currentTarget.value })}
                />
                <input
                  placeholder={t.modelBaseUrl}
                  value={modelForm.base_url}
                  onChange={(e) => setModelForm({ ...modelForm, base_url: e.currentTarget.value })}
                />
                <input
                  type="password"
                  placeholder={t.modelApiKey}
                  value={modelForm.api_key}
                  onChange={(e) => setModelForm({ ...modelForm, api_key: e.currentTarget.value })}
                />
                <div style={{ display: "flex", gap: 6 }}>
                  <button style={{ flex: 1 }} onClick={saveModel}>
                    {t.modelSave}
                  </button>
                  <button
                    className="ghost"
                    onClick={testModel}
                    disabled={!modelForm.base_url || !modelForm.model}
                  >
                    {t.modelTest}
                  </button>
                </div>
                {modelTestMsg && <div className="model-test-msg">{modelTestMsg}</div>}
              </div>
              <div className="modal-hint">{t.modelsHint}</div>
              <div style={{ marginTop: 12, display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
                <button
                  className="ghost"
                  onClick={async () => {
                    try {
                      const n = await invoke<number>("migrate_env_keys");
                      setMigrateMsg(t.migrateOk(n));
                      refreshModels();
                    } catch (e) {
                      setError(String(e));
                    }
                  }}
                >
                  {t.migrateKeys}
                </button>
                {migrateMsg && <span className="model-test-msg" style={{ margin: 0 }}>{migrateMsg}</span>}
              </div>
            </div>
            )}
            {settingsTab === "mcp" && (
            <div className="modal-section">
              {/* 实时状态：按服务器/按工具启停、授权，全部即时生效，
                  不必再改 TOML 重开会话。 */}
              <div className="modal-label">{t.mcpLiveSection}</div>
              <div className="mcp-list">
                {mcpLive.length === 0 && <div className="sidebar-empty">{t.mcpLiveEmpty}</div>}
                {mcpLive.map((s: any) => {
                  const st = s.session ?? {};
                  const on = st.enabled !== false;
                  return (
                    <div key={s.name} className="mcp-live">
                      <div className="mcp-live-head">
                        <button
                          className={`mcp-switch ${on ? "on" : ""}`}
                          title={on ? t.mcpDisable : t.mcpEnable}
                          onClick={async () => {
                            await invoke("mcp_toggle", {
                              serverName: s.name,
                              enabled: !on,
                            }).catch((e) => setError(String(e)));
                            refreshMcpLive();
                          }}
                        >
                          <span className="mcp-knob" />
                        </button>
                        <span className="mcp-live-name">{s.displayName ?? s.name}</span>
                        {st.status && <span className={`mcp-status ${st.status}`}>{st.status}</span>}
                        {s.sourceLabel && <span className="mcp-src">{s.sourceLabel}</span>}
                        {st.authRequired && (
                          <button
                            className="git-mini"
                            onClick={async () => {
                              await invoke("mcp_auth_trigger", { serverName: s.name }).catch((e) =>
                                setError(String(e)),
                              );
                              refreshMcpLive(true);
                            }}
                          >
                            {t.mcpAuth}
                          </button>
                        )}
                      </div>
                      {on && (st.tools ?? []).length > 0 && (
                        <div className="mcp-tools">
                          {st.tools.map((tool: any) => (
                            <button
                              key={tool.name}
                              className={`mcp-tool ${tool.enabled === false ? "off" : ""}`}
                              title={tool.description ?? tool.name}
                              onClick={async () => {
                                await invoke("mcp_toggle_tool", {
                                  serverName: s.name,
                                  toolName: tool.name,
                                  enabled: tool.enabled === false,
                                }).catch((e) => setError(String(e)));
                                refreshMcpLive();
                              }}
                            >
                              {tool.name}
                            </button>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>

              <div className="modal-label">{t.mcpSection}</div>
              <div className="mcp-list">
                {mcpList.length === 0 && <div className="sidebar-empty">{t.notConfigured}</div>}
                {mcpList.map((s: any) => (
                  <div key={s.name} className="mcp-item">
                    <div className="mcp-info">
                      <b>{s.name}</b>
                      <span className="mcp-detail">
                        {s.command ? `${s.command} ${s.args.join(" ")}` : s.url}
                      </span>
                    </div>
                    <button
                      className="ghost small"
                      title={t.mcpDelete}
                      onClick={async () => {
                        await invoke("mcp_config_remove", { name: s.name }).catch((e) => setError(String(e)));
                        refreshMcpConfig();
                        refreshSessions(workspace);
                      }}
                    >
                      <IconX size={13} />
                    </button>
                  </div>
                ))}
              </div>
              <div className="mcp-form">
                <input
                  placeholder={t.mcpName}
                  value={mcpForm.name}
                  onChange={(e) => setMcpForm({ ...mcpForm, name: e.currentTarget.value })}
                />
                <input
                  placeholder={t.mcpCommand}
                  value={mcpForm.command}
                  onChange={(e) => setMcpForm({ ...mcpForm, command: e.currentTarget.value })}
                />
                <input
                  placeholder={t.mcpArgs}
                  value={mcpForm.args}
                  onChange={(e) => setMcpForm({ ...mcpForm, args: e.currentTarget.value })}
                />
                <input
                  placeholder={t.mcpUrl}
                  value={mcpForm.url}
                  onChange={(e) => setMcpForm({ ...mcpForm, url: e.currentTarget.value })}
                />
                <button
                  onClick={async () => {
                    try {
                      await invoke("mcp_config_upsert", {
                        name: mcpForm.name,
                        command: mcpForm.command || null,
                        args: mcpForm.args.trim() ? mcpForm.args.trim().split(/\s+/) : [],
                        url: mcpForm.url || null,
                      });
                      setMcpForm({ name: "", command: "", args: "", url: "" });
                      refreshMcpConfig();
                      refreshSessions(workspace);
                    } catch (e) {
                      setError(String(e));
                    }
                  }}
                >
                  {t.mcpAdd}
                </button>
              </div>
            </div>
            )}
            {settingsTab === "skills" && (
            <div className="modal-section">
              <div className="modal-label">{t.skillsSection}</div>
              <div className="mcp-list">
                {skills.length === 0 && <div className="sidebar-empty">{t.skillsEmpty}</div>}
                {skills.map((sk: any) => (
                  <div
                    key={sk.path}
                    className={`mcp-item clickable ${sk.enabled === false ? "off" : ""}`}
                    onClick={() => openSkillEditor(sk.name, sk.path)}
                  >
                    <div className="mcp-info">
                      <b>{sk.name}</b>
                      <span className="mcp-detail">{sk.description}</span>
                    </div>
                    {/* 启停走引擎（写 [skills].disabled），返回全量刷新列表 */}
                    <button
                      className={`mcp-switch ${sk.enabled === false ? "" : "on"}`}
                      title={sk.enabled === false ? t.mcpEnable : t.mcpDisable}
                      onClick={async (ev) => {
                        ev.stopPropagation();
                        try {
                          const r = await invoke<any>("skills_toggle", {
                            name: sk.name,
                            enabled: sk.enabled === false,
                            workspace,
                          });
                          setSkills(
                            (r?.skills ?? []).map((x: any) => ({
                              name: x.name,
                              description: x.short_description ?? x.description ?? "",
                              path: x.path,
                              enabled: x.enabled !== false,
                              scope: typeof x.scope === "string" ? x.scope : "",
                            })),
                          );
                        } catch (e) {
                          setError(String(e));
                        }
                      }}
                    >
                      <span className="mcp-knob" />
                    </button>
                    <IconPencil size={13} className="skill-edit-icon" />
                  </div>
                ))}
              </div>
              <div className="mcp-form">
                <input
                  placeholder={t.skillsName}
                  value={skillForm.name}
                  onChange={(e) => setSkillForm({ ...skillForm, name: e.currentTarget.value })}
                />
                <input
                  placeholder={t.skillsDesc}
                  value={skillForm.description}
                  onChange={(e) => setSkillForm({ ...skillForm, description: e.currentTarget.value })}
                />
                <div style={{ display: "flex", gap: 6 }}>
                  <button
                    style={{ flex: 1 }}
                    onClick={async () => {
                      if (!skillForm.name.trim()) return;
                      try {
                        await invoke("skills_create", {
                          name: skillForm.name.trim(),
                          description: skillForm.description,
                        });
                        setSkillForm({ name: "", description: "" });
                        refreshSkills();
                      } catch (e) {
                        setError(String(e));
                      }
                    }}
                  >
                    {t.skillsCreate}
                  </button>
                  <button className="ghost" onClick={() => invoke("skills_open").catch((e) => setError(String(e)))}>
                    {t.skillsOpen}
                  </button>
                </div>
              </div>
              <div className="modal-hint">{t.skillsHint}</div>
            </div>
            )}
            {settingsTab === "hooks" && (
            <div className="modal-section">
              <div className="modal-label">{t.hooksSection}</div>
              <div className="mcp-list">
                {hooks.length === 0 && <div className="sidebar-empty">{t.hooksEmpty}</div>}
                {hooks.map((h: any, i: any) => (
                  <div key={i} className="mcp-item">
                    <div className="mcp-info">
                      <b>{h.event}</b>
                      <span className="mcp-detail">{h.command}</span>
                    </div>
                    <button
                      className="ghost small"
                      title={t.mcpDelete}
                      onClick={() => saveHooks(hooks.filter((_: any, j: any) => j !== i))}
                    >
                      <IconX size={13} />
                    </button>
                  </div>
                ))}
              </div>
              <div className="mcp-form">
                <select
                  value={hookForm.event}
                  onChange={(e) => setHookForm({ ...hookForm, event: e.currentTarget.value })}
                >
                  {["PreToolUse", "PostToolUse", "SessionStart", "Stop", "UserPromptSubmit"].map((ev: any) => (
                    <option key={ev} value={ev}>
                      {ev}
                    </option>
                  ))}
                </select>
                <input
                  placeholder={t.hooksCommand}
                  value={hookForm.command}
                  onChange={(e) => setHookForm({ ...hookForm, command: e.currentTarget.value })}
                />
                <button
                  onClick={() => {
                    if (!hookForm.command.trim()) return;
                    saveHooks([...hooks, { event: hookForm.event, command: hookForm.command.trim() }]);
                    setHookForm({ event: hookForm.event, command: "" });
                  }}
                >
                  {t.hooksAdd}
                </button>
              </div>
              <div className="modal-hint">{t.hooksHint}</div>
            </div>
            )}
            {settingsTab === "about" && (
            <>
            <div className="modal-section">
              <div className="modal-label">{t.projectMemory}</div>
              <div className="modal-body">{t.projectMemoryHelp}</div>
            </div>
            <div className="modal-section">
              <div className="modal-label">{t.configFile}</div>
              <div className="modal-body mono">{t.configHelp}</div>
            </div>
            <div className="modal-section">
              <div className="modal-label">WanCode {version ? `v${version}` : ""}</div>
              <div className="about-actions">
                <button className="ghost" onClick={runUpdate}>{t.checkUpdate}</button>
                {updateMsg && <span className="update-msg">{updateMsg}</span>}
              </div>
            </div>
            </>
            )}
            <div className="settings-footer">
              <button onClick={() => setShowSettings(false)}>{t.close}</button>
            </div>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
