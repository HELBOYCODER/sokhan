import React from "react";
import { useTranslation } from "react-i18next";
import { Cloud, Wifi } from "lucide-react";
import { useModelStore } from "@/stores/modelStore";

export const ModelsSettings: React.FC = () => {
  const { t } = useTranslation();
  const { models, currentModel } = useModelStore();
  const model = models.find((m) => m.id === (currentModel ?? "")) ?? models[0];

  return (
    <div className="space-y-4">
      <div className="rounded-xl border border-ink/80 bg-surface p-4 flex items-start gap-4">
        <div className="w-10 h-10 rounded-lg bg-accent/15 flex items-center justify-center shrink-0">
          <Cloud className="w-5 h-5 text-accent" />
        </div>
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h3 className="font-semibold text-text">
              {model?.name ?? "Google Online"}
            </h3>
            <span className="px-2 py-0.5 rounded-full bg-green-500/15 text-green-500 text-xs font-medium">
              {t("models.ready", "Ready")}
            </span>
          </div>
          <p className="mt-1 text-sm text-text/60">
            {t(
              "models.onlineDescription",
              "Sokhan transcribes with Google's online speech engine — the same engine Chrome's voice input uses. Nothing to download.",
            )}
          </p>
          <div className="mt-3 flex items-center gap-2 text-xs text-text/50">
            <Wifi className="w-3.5 h-3.5" />
            {t(
              "models.internetNote",
              "An internet connection is required while dictating.",
            )}
          </div>
        </div>
      </div>
    </div>
  );
};

export default ModelsSettings;
