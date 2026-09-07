"use client"

import { useCallback, useState } from "react"
import { Repeat } from "lucide-react"

import { SettingRow } from "@/components/shared/setting-card"
import { Switch } from "@/components/ui/switch"
import { getContinuationSettings, setContinuationSettings } from "@/lib/api"

export function useContinuationSettings() {
  const [enabled, setEnabled] = useState(false)

  const load = useCallback(async () => {
    const settings = await getContinuationSettings()
    return settings.continuable_delegation_enabled
  }, [])

  const save = useCallback(async () => {
    await setContinuationSettings({
      continuable_delegation_enabled: enabled,
    })
  }, [enabled])

  return { enabled, setEnabled, load, save }
}

interface ContinuationSettingsRowProps {
  enabled: boolean
  delegationEnabled: boolean
  loading: boolean
  onEnabledChange: (enabled: boolean) => void
  title: React.ReactNode
  description: React.ReactNode
}

export function ContinuationSettingsRow({
  enabled,
  delegationEnabled,
  loading,
  onEnabledChange,
  title,
  description,
}: ContinuationSettingsRowProps) {
  return (
    <SettingRow
      icon={Repeat}
      title={title}
      description={description}
      htmlFor="delegation-continuation-enabled"
      control={
        <Switch
          id="delegation-continuation-enabled"
          checked={enabled}
          onCheckedChange={onEnabledChange}
          disabled={loading || !delegationEnabled}
        />
      }
    />
  )
}
