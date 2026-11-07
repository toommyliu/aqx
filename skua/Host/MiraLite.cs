namespace Mira.Skua;

public sealed class MiraLite : IScriptLite
{
    public bool CharacterSelectScreen { get => Get<bool>("bCharSelect"); set => Set("bCharSelect", value); }
    public bool CustomDropsUI { get => Get<bool>("bCustomDrops"); set => Set("bCustomDrops", value); }
    public bool QuestLogTurnIns { get => Get<bool>("bQuestLog"); set => Set("bQuestLog", value); }
    public bool BattlePets { get => Get<bool>("bBattlepet"); set => Set("bBattlepet", value); }
    public bool StaticPlayerArt { get => Get<bool>("bCachePlayers"); set => Set("bCachePlayers", value); }
    public bool ChatFilter { get => Get<bool>("bChatFilter"); set => Set("bChatFilter", value); }
    public bool ChatUI { get => Get<bool>("bChatUI"); set => Set("bChatUI", value); }
    public bool AurasUI { get => Get<bool>("bAuras"); set => Set("bAuras", value); }
    public bool ColorSets { get => Get<bool>("bColorSets"); set => Set("bColorSets", value); }
    public bool DraggableDrops { get => Get<bool>("bDraggable"); set => Set("bDraggable", value); }
    public bool DisableChatScrolling { get => Get<bool>("bDisChatScroll"); set => Set("bDisChatScroll", value); }
    public bool DisableDamageNumbers { get => Get<bool>("bDisDmgDisplay"); set => Set("bDisDmgDisplay", value); }
    public bool DisableSoundFx { get => Get<bool>("bDisSoundFX"); set => Set("bDisSoundFX", value); }
    public bool DisableQuestPopup { get => Get<bool>("bDisQPopup"); set => Set("bDisQPopup", value); }
    public bool DisableQuestTracker { get => Get<bool>("bDisQTracker"); set => Set("bDisQTracker", value); }
    public bool QuestPinner { get => Get<bool>("bQuestPin"); set => Set("bQuestPin", value); }
    public bool QuestProgressNotifications { get => Get<bool>("bQuestNotif"); set => Set("bQuestNotif", value); }
    public bool VisualSkillCooldowns { get => Get<bool>("bSkillCD"); set => Set("bSkillCD", value); }
    public bool HideGroundItems { get => Get<bool>("bDisGround"); set => Set("bDisGround", value); }
    public bool HideHealingBubbles { get => Get<bool>("bDisHealBubble"); set => Set("bDisHealBubble", value); }
    public bool DisableAuraAnimations { get => Get<bool>("bDisAuraAnim"); set => Set("bDisAuraAnim", value); }
    public bool HidePlayerNames { get => Get<bool>("bHideNames"); set => Set("bHideNames", value); }
    public bool Debugger { get => Get<bool>("bDebugger"); set => Set("bDebugger", value); }
    public bool DebugPacket { get => Get<bool>("dOptions[\"debugPacket\"]"); set => Set("dOptions[\"debugPacket\"]", value); }
    public bool DisableDamageStrobe { get => Get<bool>("bDisDmgStrobe"); set => Set("bDisDmgStrobe", value); }
    public bool DisableMonsterAnimation { get => Get<bool>("bDisMonAnim"); set => Set("bDisMonAnim", value); }
    public bool DisableRedWarning { get => Get<bool>("dOptions[\"disRed\"]"); set => Set("dOptions[\"disRed\"]", value); }
    public bool DisableSelfAnimation { get => Get<bool>("bDisSelfMAnim"); set => Set("bDisSelfMAnim", value); }
    public bool DisableSkillAnimation { get => Get<bool>("bDisSkillAnim"); set => Set("bDisSkillAnim", value); }
    public bool DisableWeaponAnimation { get => Get<bool>("bDisWepAnim"); set => Set("bDisWepAnim", value); }
    public bool QuantityWarnings { get => Get<bool>("dOptions[\"termsAgree\"]"); set => Set("dOptions[\"termsAgree\"]", value); }
    public bool FreezeMonsterPosition { get => Get<bool>("bFreezeMons"); set => Set("bFreezeMons", value); }
    public bool HidePlayers { get => Get<bool>("bHidePlayers"); set => Set("bHidePlayers", value); }
    public bool HideUI { get => Get<bool>("bHideUI"); set => Set("bHideUI", value); }
    public bool InvisibleMonsters { get => Get<bool>("bHideMons"); set => Set("bHideMons", value); }
    public bool ReacceptQuest { get => Get<bool>("bReaccept"); set => Set("bReaccept", value); }
    public bool ShowNameTags { get => Get<bool>("dOptions[\"showNames\"]"); set => Set("dOptions[\"showNames\"]", value); }
    public bool ShowShadows { get => Get<bool>("dOptions[\"showShadows\"]"); set => Set("dOptions[\"showShadows\"]", value); }
    public bool HideGuildNamesOnly { get => Get<bool>("dOptions[\"hideGuild\"]"); set => Set("dOptions[\"hideGuild\"]", value); }
    public bool HideYourNameOnly { get => Get<bool>("dOptions[\"hideSelf\"]"); set => Set("dOptions[\"hideSelf\"]", value); }
    public bool ShowYourGroundItemOnly { get => Get<bool>("dOptions[\"groundSelf\"]"); set => Set("dOptions[\"groundSelf\"]", value); }
    public bool ShowYourAuraAnimationOnly { get => Get<bool>("dOptions[\"auraAnimSelf\"]"); set => Set("dOptions[\"auraAnimSelf\"]", value); }
    public bool ShowMonsterType { get => Get<bool>("bMonsType"); set => Set("bMonsType", value); }
    public bool SmoothBackground { get => Get<bool>("bSmoothBG"); set => Set("bSmoothBG", value); }
    public bool UntargetDead { get => Get<bool>("bUntargetDead"); set => Set("bUntargetDead", value); }
    public bool UntargetSelf { get => Get<bool>("bUntargetSelf"); set => Set("bUntargetSelf", value); }
    readonly System.Collections.Concurrent.ConcurrentDictionary<string, object> preferences = new();
    public T? Get<T>(string optionName) => preferences.TryGetValue(optionName, out var value) ? (T)value : default;
    public void Set<T>(string optionName, T value) { if (value is null) preferences.TryRemove(optionName, out _); else preferences[optionName] = value; }
}
