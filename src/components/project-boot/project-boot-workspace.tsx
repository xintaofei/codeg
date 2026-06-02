"use client"

import { useTranslations } from "next-intl"
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs"
import { ShadcnLauncher } from "./shadcn/shadcn-launcher"
import { HyperframesLauncher } from "./hyperframes/hyperframes-launcher"

export function ProjectBootWorkspace() {
  const t = useTranslations("ProjectBoot")

  return (
    <Tabs defaultValue="shadcn" className="flex h-full flex-col gap-0">
      <div className="shrink-0 border-b px-4 py-2">
        <TabsList>
          <TabsTrigger value="shadcn">{t("tabs.shadcn")}</TabsTrigger>
          <TabsTrigger value="hyperframes">{t("tabs.hyperframes")}</TabsTrigger>
        </TabsList>
      </div>

      <TabsContent value="shadcn" className="min-h-0 flex-1">
        <ShadcnLauncher />
      </TabsContent>

      <TabsContent value="hyperframes" className="min-h-0 flex-1">
        <HyperframesLauncher />
      </TabsContent>
    </Tabs>
  )
}
