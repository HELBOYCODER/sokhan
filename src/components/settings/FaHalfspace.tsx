import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface FaHalfspaceProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const FaHalfspace: React.FC<FaHalfspaceProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const enabled = getSetting("fa_halfspace_enabled") ?? true;

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(nextEnabled) =>
          updateSetting("fa_halfspace_enabled", nextEnabled)
        }
        isUpdating={isUpdating("fa_halfspace_enabled")}
        label={t("settings.advanced.faHalfspace.title")}
        description={t("settings.advanced.faHalfspace.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  },
);
