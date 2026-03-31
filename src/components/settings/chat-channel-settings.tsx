"use client"

import { useTranslations } from "next-intl"

import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { ChannelListTab } from "./channel-list-tab"
import { ChannelCommandsTab } from "./channel-commands-tab"
import { ChannelEventsTab } from "./channel-events-tab"

export function ChatChannelSettings() {
  const t = useTranslations("ChatChannelSettings")

  return (
    <div className="h-full overflow-auto">
      <Tabs defaultValue="channels" className="w-full space-y-4">
        <section className="space-y-3">
          <div>
            <h1 className="text-sm font-semibold">{t("sectionTitle")}</h1>
            <p className="text-sm text-muted-foreground">
              {t("sectionDescription")}
            </p>
          </div>
          <TabsList>
            <TabsTrigger value="channels">{t("tabs.channels")}</TabsTrigger>
            <TabsTrigger value="commands">{t("tabs.commands")}</TabsTrigger>
            <TabsTrigger value="events">{t("tabs.events")}</TabsTrigger>
          </TabsList>
        </section>

        <TabsContent value="channels" className="mt-0">
          <ChannelListTab />
        </TabsContent>
        <TabsContent value="commands" className="mt-0">
          <ChannelCommandsTab />
        </TabsContent>
        <TabsContent value="events" className="mt-0">
          <ChannelEventsTab />
        </TabsContent>
      </Tabs>
    </div>
  )
}
