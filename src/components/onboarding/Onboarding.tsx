import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Mic } from "lucide-react";
import SokhanTextLogo from "../icons/SokhanTextLogo";
import { useModelStore } from "../../stores/modelStore";

interface OnboardingProps {
  onModelSelected: () => void;
  preview?: boolean;
}

const ONLINE_MODEL_ID = "google-online";

const Onboarding: React.FC<OnboardingProps> = ({
  onModelSelected,
  preview = false,
}) => {
  const { t } = useTranslation();
  const { selectModel } = useModelStore();
  const [busy, setBusy] = useState(false);

  const handleStart = async () => {
    if (preview || busy) return;
    setBusy(true);
    const success = await selectModel(ONLINE_MODEL_ID);
    if (success) {
      onModelSelected();
    } else {
      toast.error(t("onboarding.errors.selectModel"));
      setBusy(false);
    }
  };

  return (
    <div className="h-screen w-full flex flex-col p-6 gap-6">
      <div className="flex flex-col items-center gap-3 shrink-0">
        <SokhanTextLogo width={200} />
        <p className="text-text/70 max-w-md font-medium mx-auto text-center">
          {t("onboarding.subtitle")}
        </p>
      </div>

      <div className="max-w-[480px] w-full mx-auto flex-1 flex flex-col justify-center gap-6">
        <div className="rounded-2xl border border-ink/80 bg-surface p-6 flex flex-col items-center gap-4 text-center">
          <div className="w-14 h-14 rounded-full bg-accent/15 flex items-center justify-center">
            <Mic className="w-7 h-7 text-accent" />
          </div>
          <div>
            <h2 className="text-lg font-semibold text-text">
              {t("onboarding.onlineTitle")}
            </h2>
            <p className="mt-1 text-sm text-text/60">
              {t("onboarding.onlineDescription")}
            </p>
          </div>
          <button
            type="button"
            onClick={handleStart}
            disabled={busy}
            className="px-6 py-2.5 rounded-xl bg-accent text-ink font-semibold hover:bg-accent-hover transition-colors disabled:opacity-50"
          >
            {busy ? t("onboarding.starting") : t("onboarding.getStart")}
          </button>
        </div>
      </div>
    </div>
  );
};

export default Onboarding;
