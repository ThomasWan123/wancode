import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/// 首次运行向导（v0.12.1 引入，v0.13 拆分自 App.tsx）。
///
/// 状态自包含：与设置页的一键配置各持一份 quick* 状态，互不纠缠。
/// 注意零模型时引擎不可启动（v0.12.2 后端不变量）——向导链路全部走
/// 纯 Rust 命令，不需要引擎在场。
export interface OnboardingProps {
  t: Record<string, any>;
  onConfigured: () => void; // 刷新模型列表等
  onOpenFolder: () => void; // 第二步：打开工作区
  onCustomEndpoint: () => void; // 转设置页手工配置
  onClose: () => void;
}

const PRESETS = [
  ["glm-coding", "GLM Coding Plan", "presetGlmCoding"],
  ["glm-open", "智谱开放平台", "presetGlmOpen"],
  ["deepseek", "DeepSeek", "presetDeepseek"],
] as const;

export function OnboardingWizard({
  t,
  onConfigured,
  onOpenFolder,
  onCustomEndpoint,
  onClose,
}: OnboardingProps) {
  const [step, setStep] = useState<1 | 2>(1);
  const [preset, setPreset] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState("");

  async function testAndSave() {
    setBusy(true);
    setResult("");
    try {
      const r = await invoke<any>("provider_quick_setup", { preset, apiKey: key });
      setKey("");
      onConfigured();
      setResult(
        `✅ ${t.quickDone}${(r.models ?? []).map((m: any) => m.name).join("、")}${
          r.mcpSeeded ? ` · ${t.quickMcpSeeded}` : ""
        }`,
      );
      setStep(2);
    } catch (e) {
      setResult(`❌ ${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="modal-mask">
      <div className="modal ob-modal">
        <div className="ob-title">{t.obWelcome}</div>
        <div className="ob-sub">{step === 1 ? t.obStep1Sub : t.obStep2Sub}</div>
        {step === 1 && (
          <>
            <div className="preset-cards">
              {PRESETS.map(([id, label, hintKey]) => (
                <button
                  key={id}
                  className={`preset-card ${preset === id ? "active" : ""}`}
                  onClick={() => setPreset(preset === id ? "" : id)}
                >
                  <b>{label}</b>
                  <span>{t[hintKey]}</span>
                </button>
              ))}
            </div>
            {preset && (
              <div className="quick-key-row">
                <input
                  type="password"
                  placeholder={t.quickKeyPlaceholder}
                  value={key}
                  onChange={(e) => setKey(e.target.value)}
                  autoFocus
                />
                <button disabled={!key.trim() || busy} onClick={testAndSave}>
                  {busy ? t.quickTesting : t.quickGo}
                </button>
              </div>
            )}
            {result && <div className="quick-result">{result}</div>}
            <div className="ob-foot">
              <button className="ghost" onClick={onCustomEndpoint}>
                {t.obCustom}
              </button>
              <button className="ghost" onClick={onClose}>
                {t.obSkip}
              </button>
            </div>
          </>
        )}
        {step === 2 && (
          <>
            {result && <div className="quick-result">{result}</div>}
            <button className="ob-open" onClick={onOpenFolder}>
              📁 {t.obOpenFolder}
            </button>
            <div className="ob-foot">
              <button className="ghost" onClick={onClose}>
                {t.obSkip}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
