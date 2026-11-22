import { useEffect, useState } from "react";
import { desktop } from "../../application/desktop";
import { EFFORT_LABELS, removalQuestion, runsLocally, supportsEffort, type LlmProvider, type Model, type ProviderModels } from "../../domain/model";
import { askToConfirm } from "../../infrastructure/confirm";
import { Button } from "../ui/base/Button.js";
import { Card } from "../ui/base/Card.js";
import { Choice, Field } from "../ui/base/Field.js";
import { Input } from "../ui/base/Input.js";
import { CardRow, Status, SubTitle } from "./Parts";
import { t } from "../../domain/i18n";

/** The model choice that opens a field for a model ID of the person's own. */
const OTHER_MODEL = "\u0000other";

export function LlmPage(props: {
  models: Model[];
  onChanged: () => Promise<void>;
  onError: (message: string) => void;
}) {
  const [providers, setProviders] = useState<LlmProvider[]>([]);
  const [providerId, setProviderId] = useState("openai");
  const [modelId, setModelId] = useState("");
  const [baseUrl, setBaseUrl] = useState("http://127.0.0.1:1234");
  const [secret, setSecret] = useState("");
  const [effort, setEffort] = useState("");
  const [studioModels, setStudioModels] = useState<{ id: string; label: string }[]>([]);
  // What a cloud provider listed for a key, and whether the person is typing a model ID of their own.
  const [listed, setListed] = useState<ProviderModels | null>(null);
  const [typing, setTyping] = useState(false);
  // The key in the form once the person has stopped typing it, so the provider is asked once and not per keystroke.
  const [typedKey, setTypedKey] = useState("");
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [keyDraft, setKeyDraft] = useState("");
  const provider = providers.find((item) => item.id === providerId) ?? providers[0];

  useEffect(() => {
    void desktop.admin.providers().then((next) => {
      setProviders(next);
      const first = next[0];
      if (!first) return;
      setProviderId(first.id);
      setModelId(first.models[0]?.id ?? "");
      setBaseUrl(first.baseUrl);
    }).catch((reason: unknown) => props.onError(reason instanceof Error ? reason.message : t("Could not load models")));
  }, []);

  useEffect(() => {
    if (!runsLocally(providerId)) return;
    const url = baseUrl.trim();
    // A slow answer from the server chosen before must not fill in the models of the one chosen now.
    let current = true;
    setStudioModels([]);
    void desktop.admin.localModels(providerId, url)
      .then((next) => {
        if (!current) return;
        setStudioModels(next);
        setModelId((chosen) => chosen || next[0]?.id || "");
      })
      .catch(() => {
        if (current) setStudioModels([]);
      });
    return () => {
      current = false;
    };
  }, [providerId, baseUrl]);

  useEffect(() => {
    const key = secret.trim();
    if (!key) return setTypedKey("");
    const timer = setTimeout(() => setTypedKey(key), 600);
    return () => clearTimeout(timer);
  }, [secret]);

  // The app has no list of a provider's models: the provider is asked, with its saved key or the one being typed, and
  // again when a key is saved. Until it answers, or when there is no key yet, the model ID can be typed.
  const keyed = props.models.filter((item) => item.provider === providerId && item.hasSecret).length;
  useEffect(() => {
    if (runsLocally(providerId) || providers.length === 0) return;
    let current = true;
    setListed(null);
    void desktop.admin.providerModels(providerId, typedKey)
      .then((next) => {
        if (!current) return;
        setListed(next);
        setModelId((chosen) => (next.models.length === 0 || next.models.some((item) => item.id === chosen) ? chosen : next.models[0].id));
      })
      .catch(() => {
        if (current) setListed(null);
      });
    return () => {
      current = false;
    };
  }, [providerId, keyed, providers.length, typedKey]);

  function chooseProvider(id: string) {
    const next = providers.find((item) => item.id === id);
    setTyping(false);
    setProviderId(id);
    setModelId(next?.models[0]?.id ?? "");
    setBaseUrl(next?.baseUrl ?? "");
    setSecret("");
    setTypedKey("");
  }

  async function add() {
    try {
      // A model its provider listed keeps what the provider said of it: its name, its context window, what it accepts.
      const chosen = listed?.source === "provider" ? listed.models.find((item) => item.id === modelId) : undefined;
      await desktop.admin.createModel({
        provider: providerId,
        model: modelId.trim(),
        name: chosen && chosen.label !== chosen.id ? chosen.label : undefined,
        contextTokens: chosen?.contextTokens,
        capabilities: chosen?.capabilities,
        baseUrl: runsLocally(providerId) ? baseUrl : undefined,
        secret: secret || undefined,
        effort: supportsEffort(providerId) && effort ? effort : undefined,
      });
      setSecret("");
      setTypedKey("");
      props.onError("");
      await props.onChanged();
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not connect the model"));
    }
  }

  async function bindKey(id: string) {
    try {
      await desktop.admin.updateModelKey(id, keyDraft.trim());
      setEditingKey(null);
      setKeyDraft("");
      props.onError("");
      await props.onChanged();
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not save the API key"));
    }
  }

  const choices = runsLocally(providerId) ? studioModels : listed?.models ?? provider?.models ?? [];
  const needsKey = provider?.needsKey ?? true;
  const savedKey = props.models.some((item) => item.provider === providerId && item.hasSecret);

  return (
    <>
      <SubTitle>{t("Connected models")}</SubTitle>
      {props.models.length === 0 ? (
        <p className="mb-3.5 text-[12.5px] text-muted-foreground">{t("No models connected yet. Add a cloud model below, or one from a model server on this computer.")}</p>
      ) : (
        props.models.map((item) => {
          return (
            <Card key={item.id}>
              <CardRow>
                <div>
                  <div className="flex items-center gap-2">
                    <strong>{item.name}</strong>
                  </div>
                  <div className="text-[12.5px] text-muted-foreground">{providerLabel(providers, item)}{item.model ? ` · ${item.model}` : ""}</div>
                </div>
                <div className="flex items-center gap-2">
                  {supportsEffort(item.provider) ? (
                    <div title={(item.provider && EFFORT_HINTS[item.provider]) || t("How much the model reasons before answering")}>
                      <Choice
                        className="min-w-33"
                        ariaLabel={t("Effort for {name}", { name: item.name })}
                        value={item.effort || DEFAULT_EFFORT}
                        options={[{ value: DEFAULT_EFFORT, label: t("Default effort") }, ...Object.entries(EFFORT_LABELS).map(([value, label]) => ({ value, label: t("{level} effort", { level: EFFORT_NAMES[value] ?? label }) }))]}
                        onChange={async (next) => {
                          try {
                            await desktop.admin.updateModelEffort(item.id, next === DEFAULT_EFFORT ? null : next);
                            await props.onChanged();
                          } catch (err) {
                            props.onError(err instanceof Error ? err.message : t("Could not change the effort"));
                          }
                        }}
                      />
                    </div>
                  ) : null}
                  <Status
                    ok={runsLocally(item.provider) || item.provider === "claude-code" || item.provider === "codex" || item.hasSecret}
                    label={runsLocally(item.provider) ? t("Local Server") : item.provider === "claude-code" ? t("Claude plan") : item.provider === "codex" ? t("ChatGPT plan") : item.hasSecret ? t("Key stored") : t("No key")}
                  />
                  {!runsLocally(item.provider) && item.provider !== "claude-code" && item.provider !== "codex" && editingKey !== item.id ? (
                    <Button
                      variant="outline" size="small"
                      type="button"
                      onClick={(e) => {
                        e.stopPropagation();
                        setEditingKey(item.id);
                        setKeyDraft("");
                      }}
                    >
                      {item.hasSecret ? t("Replace Key") : t("Add Key")}
                    </Button>
                  ) : null}
                  <Button
                    variant="ghost" size="small" className="text-muted-foreground hover:bg-destructive/12 hover:text-destructive"
                    type="button"
                    title={t("Remove model")}
                    onClick={async (e) => {
                      e.stopPropagation();
                      try {
                        const users = (await desktop.catalog.listAgents()).filter((agent) => agent.modelConnectionId === item.id).map((agent) => agent.name);
                        const replacement = props.models.find((model) => model.id !== item.id) ?? null;
                        if (!await askToConfirm(removalQuestion(item.name, users, "model", replacement?.name ?? null))) return;
                        await desktop.admin.deleteModel(item.id, users.length ? (replacement?.id ?? "none") : undefined);
                        await props.onChanged();
                      } catch (err) {
                        props.onError(err instanceof Error ? err.message : t("Failed to remove model"));
                      }
                    }}
                  >
                    {t("Remove")}
                  </Button>
                </div>
              </CardRow>
              {editingKey === item.id ? (
                <form
                  className="mt-2.5 flex items-center gap-2 border-t border-border pt-2.5"
                  onClick={(e) => e.stopPropagation()}
                  onSubmit={(e) => {
                    e.preventDefault();
                    void bindKey(item.id);
                  }}
                >
                  <Input
                    type="password"
                    autoFocus
                    autoComplete="off"
                    aria-label={t("API key for {name}", { name: item.name })}
                    placeholder={t("Paste {provider} API key…", { provider: providerLabel(providers, item) })}
                    value={keyDraft}
                    onChange={(event) => setKeyDraft(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Escape") setEditingKey(null);
                    }}
                  />
                  <Button variant="outline" size="small" type="button" onClick={() => setEditingKey(null)}>{t("Cancel")}</Button>
                  <Button variant="default" size="small" type="submit" disabled={!keyDraft.trim()}>{t("Save Key")}</Button>
                </form>
              ) : null}
            </Card>
          );
        })
      )}

      <SubTitle className="mt-5">{t("Connect a model")}</SubTitle>
      <Card className="gap-3.5">
        <div className="grid grid-cols-2 gap-3 max-[860px]:grid-cols-1">
          <Choice label={t("Provider")} value={providerId} options={providers.map((item) => ({ value: item.id, label: item.label }))} onChange={chooseProvider} />
          {runsLocally(providerId) && choices.length === 0 ? (
            <Field label={t("Model")}>
              <Input id="llm-model" value={modelId} placeholder={t("Model ID, as the server names it")} onChange={(event) => setModelId(event.target.value)} />
            </Field>
          ) : choices.length === 0 ? (
            <Field label={t("Model")}>
              <Input id="llm-model" value={modelId} placeholder={t("Model ID, as the provider names it")} onChange={(event) => setModelId(event.target.value)} />
            </Field>
          ) : typing ? (
            <Field label={t("Model")}>
              <div className="flex gap-2">
                <Input id="llm-model" value={modelId} placeholder={t("Model ID, as the provider names it")} onChange={(event) => setModelId(event.target.value)} />
                <Button variant="outline" size="small" type="button" onClick={() => { setTyping(false); setModelId(choices[0]?.id ?? ""); }}>{t("Back to the list")}</Button>
              </div>
            </Field>
          ) : (
            <Choice
              label={t("Model")}
              value={modelId}
              options={[...choices.map((item) => ({ value: item.id, label: item.label })), ...(runsLocally(providerId) ? [] : [{ value: OTHER_MODEL, label: t("Other model ID…") }])]}
              onChange={(next) => {
                if (next !== OTHER_MODEL) return setModelId(next);
                setTyping(true);
                setModelId("");
              }}
            />
          )}
        </div>
        {!runsLocally(providerId) && listed ? (
          <p className="m-0 text-[12.5px] leading-snug text-muted-foreground" title={listed.note}>
            {listed.source === "provider"
              ? typedKey
                ? t("Listed by {provider} for the API key you entered.", { provider: provider?.label ?? "" })
                : t("Listed by {provider} for your saved API key.", { provider: provider?.label ?? "" })
              : listed.source === "program"
                ? t("These names follow the program's newest models, so none goes out of date. Choose Other model ID to use any model the program accepts.")
                : typedKey || savedKey
                  ? t("{provider} did not list its models for this key. Type the model ID as {provider} names it.", { provider: provider?.label ?? "" })
                  : t("Enter your API key below and {provider} lists the models it can use. You can also type a model ID.", { provider: provider?.label ?? "" })}
          </p>
        ) : null}

        {supportsEffort(providerId) ? (
          <div className="flex flex-col gap-1.5">
            <Choice
              label={t("Effort")}
              value={effort || DEFAULT_EFFORT}
              options={[{ value: DEFAULT_EFFORT, label: t("Model default") }, ...Object.entries(EFFORT_LABELS).map(([value, label]) => ({ value, label: EFFORT_NAMES[value] ?? label }))]}
              onChange={(next) => setEffort(next === DEFAULT_EFFORT ? "" : next)}
            />
            <p className="m-0 text-[12.5px] leading-snug text-muted-foreground">
              {EFFORT_HINTS[providerId] ?? t("How much the model reasons before answering. Higher is slower and costs more.")}
              {providerId === "claude-code" ? " " + t("Runs through the official Claude Code program on your own Claude login (Settings → General → Claude Code). The app never sends API requests for it, takes no API key, and does not schedule it.") : ""}
              {providerId === "codex" ? " " + t("Runs through the official Codex on your own ChatGPT login (Settings → General → Codex). No API key.") : ""}
            </p>
          </div>
        ) : null}

        {runsLocally(providerId) ? (
          <div className="flex flex-col gap-1.5">
            <Field label={t("Server address")}>
              <Input id="llm-host" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} />
            </Field>
            <p className="m-0 text-[12.5px] leading-snug text-muted-foreground">
              {t("Any model server on this computer with OpenAI-compatible endpoints: LM Studio (port 1234), Ollama (11434), vLLM and llama.cpp (8000), mlx_lm.server (8080). The models it has loaded are listed here; to be the research assistant, a model needs tool calling and a context of 32K or more.")}
            </p>
          </div>
        ) : null}

        {needsKey ? (
          <Field label={t("API Key")}>
            <Input
              id="llm-key"
              type="password"
              value={secret}
              placeholder={savedKey ? t("Using your saved {provider} key — paste to replace", { provider: provider?.label ?? "" }) : t("Paste provider API key…")}
              autoComplete="off"
              onChange={(event) => setSecret(event.target.value)}
            />
          </Field>
        ) : null}

        <div className="flex items-center gap-2">
          <Button
            variant="default"
            type="button"
            disabled={!modelId.trim() || (needsKey && !savedKey && !secret.trim())}
            onClick={() => void add()}
          >
            {t("Connect Model")}
          </Button>
        </div>
      </Card>
    </>
  );
}

/** The effort choice that stands for the model's own default, which is stored as no effort at all. */
const DEFAULT_EFFORT = "default";

/** What the effort setting does on each provider. */
const EFFORT_HINTS: Record<string, string> = {
  claude: t("Claude's effort setting (output_config.effort). Models without effort support ignore it."),
  "claude-code": t("Claude Code's effort level."),
  codex: t("Codex reasoning effort."),
  openai: t("OpenAI reasoning effort for o-series and GPT-5 models (Extra High and Max use High). Other models ignore it."),
  gemini: t("Gemini thinking level (3.x and 4) or thinking budget (2.5)."),
};

/** Each effort level's name as shown, by its stored value. */
const EFFORT_NAMES: Record<string, string> = { low: t("Low"), medium: t("Medium"), high: t("High"), xhigh: t("Extra High"), max: t("Max") };

function providerLabel(providers: LlmProvider[], model: Model) {
  return providers.find((item) => item.id === model.provider)?.label ?? model.provider ?? t("Model");
}
