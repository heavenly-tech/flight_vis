import { bootI18n } from "../i18n/boot";

export async function startClubPage(opts?: {
  switcher?: string;
  persistUser?: boolean;
}): Promise<void> {
  await bootI18n({
    switcher: opts?.switcher ?? "#lang-slot",
    persistUser: opts?.persistUser,
  });
}