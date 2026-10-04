import type { JsonValue, ModuleSetting, SettingsView, SettingView } from "@/lib/api";
import type { Locale } from "@/i18n";
import { moduleField } from "@/lib/moduleSettings";

const languages: Locale[] = ["en", "ru", "de", "fr", "es", "zh"];
const groups = [
  { id: "squid.population", titles: ["Population", "Население мира", "Bevölkerung", "Population", "Población", "世界人口"] },
  { id: "squid.party", titles: ["Groups", "Группы", "Gruppen", "Groupes", "Grupos", "队伍"] },
  { id: "squid.combat", titles: ["Combat", "Бой", "Kampf", "Combat", "Combate", "战斗"] },
  { id: "squid.activities", titles: ["Dungeons and PvP", "Подземелья и PvP", "Dungeons und PvP", "Donjons et JcJ", "Mazmorras y JcJ", "地下城与PvP"] },
  { id: "squid.chat", titles: ["Chat", "Общение", "Chat", "Discussion", "Chat", "聊天"] },
  { id: "squid.other", titles: ["Other settings", "Другие настройки", "Weitere Einstellungen", "Autres réglages", "Otros ajustes", "其他设置"] },
];

function group(key: string, curated: boolean) {
  if (!curated) return "squid.other";
  if (/Talk|Emote|Suggest/.test(key)) return "squid.chat";
  if (/Lfg|JoinBG/.test(key)) return "squid.activities";
  if (/Smart|SpecRotations|Interrupt/.test(key)) return "squid.combat";
  if (/AddedBots|BotAutologin$|Invite|AddClassCommand|KeepAlts|Recruit|FollowDistance/.test(key) && !key.endsWith("RandomBotAutologin")) return "squid.party";
  return "squid.population";
}

/** Use the same settings screen as Companions, backed only by shipped SQUID options. */
export function squidSettingsView(items: ModuleSetting[], locale: Locale): SettingsView {
  const settings: SettingView[] = items.filter(s => s.key !== "AiPlayerbot.Enabled" && !s.key.startsWith("PlayerbotsDatabase") && !s.key.startsWith("Playerbots.Updates.")).map(s => {
    const field = moduleField(s.key, locale);
    const type = field?.type === "bool" || field?.type === "int" || field?.type === "float" || field?.type === "enum" ? field.type : "string";
    const decode = (raw: string): JsonValue => {
      const value = raw.trim().replace(/^"|"$/g, "");
      if (type === "bool") return !["0", "false", "no", "off"].includes(value.toLowerCase());
      if (type === "int" || type === "float") return Number(value);
      return value;
    };
    const value = decode(s.value), defaultValue = decode(s.default ?? s.value);
    return {
      key: s.key, type, title: field?.title ?? s.key, description: field?.description ?? s.doc,
      category: group(s.key, !!field), advanced: !field || !!field.advanced,
      min: field?.min, max: field?.max, options: field?.options ?? [],
      value, default: defaultValue, is_default: value === defaultValue, present: true, problem: null,
      drift: false, restartRequired: "world", dangerous: false,
    };
  });
  return { scope: "bots", categories: groups.map(g => ({ id: g.id, title: g.titles[languages.indexOf(locale)] })), settings, unknown_keys: 0, drift_keys: [], files: ["Core/configs/modules/playerbots.conf"] };
}
