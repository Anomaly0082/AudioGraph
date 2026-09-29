import { useEffect, useRef, useState } from "react";
import {
  loadedAiDraft,
  normalizeAiConfig,
  sameAiConfig,
  type AiConfig,
  type AiSettingsLoad,
  type AiSettingsWrite,
} from "../ai-model";
import { formatError } from "../model";

export type AiInvoke = <T>(
  command: string,
  args: Record<string, unknown>,
) => Promise<T>;
export const initialAiConfig: AiConfig = {
  baseUrl: "https://api.openai.com/v1",
  model: "",
  apiKey: "",
};

export function useAiSettings(invokeAi: AiInvoke) {
  const [config, setConfig] = useState<AiConfig>(initialAiConfig);
  const [savedConfig, setSavedConfig] = useState<AiConfig | null>(null);
  const [settingsPath, setSettingsPath] = useState("");
  const [settingsLoading, setSettingsLoading] = useState(true);
  const [settingsBusy, setSettingsBusy] = useState(false);
  const [settingsError, setSettingsError] = useState("");
  const [settingsNotice, setSettingsNotice] = useState("");
  const mountedRef = useRef(true);
  const settingsBusyRef = useRef(false);
  const settingsEditRevisionRef = useRef(0);
  const settingsMutationRef = useRef(0);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);
  useEffect(() => {
    let active = true;
    const editRevision = settingsEditRevisionRef.current;
    const mutation = settingsMutationRef.current;
    void invokeAi<AiSettingsLoad>("ai_load_settings", {})
      .then((loaded) => {
        if (!active || settingsMutationRef.current !== mutation) return;
        const loadedConfig =
          loaded.config === null ? null : normalizeAiConfig(loaded.config);
        setSettingsPath(loaded.path);
        setSavedConfig(loadedConfig);
        setConfig((current) =>
          loadedAiDraft(
            current,
            loadedConfig,
            settingsEditRevisionRef.current !== editRevision,
          ),
        );
        setSettingsError("");
      })
      .catch((reason) => {
        if (active && settingsMutationRef.current === mutation)
          setSettingsError(`读取本机配置失败：${formatError(reason)}`);
      })
      .finally(() => {
        if (active) setSettingsLoading(false);
      });
    return () => {
      active = false;
    };
  }, [invokeAi]);

  function editConfig(change: (current: AiConfig) => AiConfig) {
    settingsEditRevisionRef.current++;
    setConfig(change);
    setSettingsNotice("");
  }

  async function saveSettings(safeConfig: AiConfig): Promise<boolean> {
    if (settingsBusyRef.current || settingsLoading) return false;
    settingsBusyRef.current = true;
    settingsMutationRef.current++;
    setSettingsBusy(true);
    setSettingsError("");
    setSettingsNotice("");
    const editRevision = settingsEditRevisionRef.current;
    try {
      const result = await invokeAi<AiSettingsWrite>("ai_save_settings", {
        config: safeConfig,
      });
      if (!mountedRef.current) return false;
      setSettingsPath(result.path);
      setSettingsLoading(false);
      setSavedConfig(safeConfig);
      if (settingsEditRevisionRef.current === editRevision)
        setConfig(safeConfig);
      setSettingsNotice("配置已保存到本机。");
      return true;
    } catch (reason) {
      if (mountedRef.current)
        setSettingsError(`保存本机配置失败：${formatError(reason)}`);
      return false;
    } finally {
      settingsBusyRef.current = false;
      if (mountedRef.current) setSettingsBusy(false);
    }
  }

  function saveCurrentSettings(workflowBusy = false) {
    if (settingsBusyRef.current || settingsLoading || workflowBusy) return;
    let safeConfig: AiConfig;
    try {
      safeConfig = normalizeAiConfig(config);
    } catch (reason) {
      setSettingsError(`保存本机配置失败：${formatError(reason)}`);
      return;
    }
    void saveSettings(safeConfig);
  }

  async function clearSavedSettings(workflowBusy = false) {
    if (settingsBusyRef.current || settingsLoading || workflowBusy) return;
    settingsBusyRef.current = true;
    settingsMutationRef.current++;
    setSettingsBusy(true);
    setSettingsError("");
    setSettingsNotice("");
    try {
      const result = await invokeAi<AiSettingsWrite>("ai_clear_settings", {});
      if (!mountedRef.current) return;
      setSettingsPath(result.path);
      setSettingsLoading(false);
      setSavedConfig(null);
      settingsEditRevisionRef.current++;
      setConfig((current) => ({ ...current, apiKey: "" }));
      setSettingsNotice("已删除本机保存的配置，并清除当前输入的 API Key。");
    } catch (reason) {
      if (mountedRef.current)
        setSettingsError(`清除本机配置失败：${formatError(reason)}`);
    } finally {
      settingsBusyRef.current = false;
      if (mountedRef.current) setSettingsBusy(false);
    }
  }

  const configStatus = settingsLoading
    ? "正在读取本机配置…"
    : savedConfig === null
      ? sameAiConfig(config, initialAiConfig)
        ? "本机尚无已保存配置"
        : "有未保存的修改"
      : sameAiConfig(config, savedConfig)
        ? "配置已保存"
        : "有未保存的修改";

  return {
    config,
    savedConfig,
    settingsPath,
    settingsLoading,
    settingsBusy,
    settingsError,
    settingsNotice,
    configStatus,
    editConfig,
    saveSettings,
    saveCurrentSettings,
    clearSavedSettings,
  };
}

export type AiSettings = ReturnType<typeof useAiSettings>;
