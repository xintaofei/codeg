"use client"

import { Suspense } from "react"
import { useSearchParams } from "next/navigation"
import { useTranslations } from "next-intl"
import { PetWindow } from "./_components/PetWindow"

function PetPageInner() {
  const t = useTranslations("Pet")
  const params = useSearchParams()
  const petId = params.get("petId")

  if (!petId) {
    return (
      <div
        className="flex h-screen w-screen items-center justify-center text-xs text-muted-foreground"
        style={{ background: "transparent" }}
      >
        {t("missingPetIdParam")}
      </div>
    )
  }

  return <PetWindow petId={petId} />
}

export default function PetPage() {
  return (
    <Suspense fallback={null}>
      <PetPageInner />
    </Suspense>
  )
}
