//! Ported from
//! https://github.com/JSamir/wc3-replay-parser/blob/master/src/main/java/com/w3gdata/parser/action/Actions.java

use flo_util::binary::*;
use flo_util::{BinDecode, BinEncode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, BinEncode, BinDecode)]
#[bin(enum_repr(u8))]
pub enum ActionTypeId {
  #[bin(value = 0x01)]
  PauseGame,
  #[bin(value = 0x02)]
  ResumeGame,
  #[bin(value = 0x03)]
  SetGameSpeed,
  #[bin(value = 0x04)]
  IncGameSpeed,
  #[bin(value = 0x05)]
  DecGameSpeed,
  #[bin(value = 0x06)]
  SaveGame,
  #[bin(value = 0x07)]
  SaveGameFinished,
  #[bin(value = 0x10)]
  UnitOrderBasic,
  #[bin(value = 0x11)]
  UnitOrderTargetPoint,
  #[bin(value = 0x12)]
  UnitOrderTargetImage,
  #[bin(value = 0x13)]
  UnitOrderTargetImage2,
  #[bin(value = 0x14)]
  UnitOrderTargetImageFogged,
  #[bin(value = 0x15)]
  UnitOrderTargetImageFogged2,
  #[bin(value = 0x16)]
  ChangeSelection,
  #[bin(value = 0x17)]
  AssignGroupHotkey,
  #[bin(value = 0x18)]
  SelectGroupHotkey,
  #[bin(value = 0x19)]
  SelectSubgroup,
  #[bin(value = 0x1A)]
  RefreshSubgroup,
  #[bin(value = 0x1B)]
  UnitSelectionEvent,
  #[bin(value = 0x1C)]
  SelectableSelectionModify,
  #[bin(value = 0x1D)]
  CancelHeroRevival,
  #[bin(value = 0x1E)]
  RemoveUnitFromBuildingQueue,
  #[bin(value = 0x50)]
  ChangeAllyOptions,
  #[bin(value = 0x51)]
  TransferResources,
  #[bin(value = 0x60)]
  MapTriggerChatCommand,
  #[bin(value = 0x61)]
  EscPressed,
  #[bin(value = 0x62)]
  ResumeTriggerExec,
  #[bin(value = 0x63)]
  TriggerSyncReady,
  #[bin(value = 0x64)]
  TrackableHit,
  #[bin(value = 0x65)]
  TrackableTrack,
  #[bin(value = 0x66)]
  EnterChooseHeroSkillSubmenu,
  #[bin(value = 0x67)]
  EnterChooseBuildingSubmenu,
  #[bin(value = 0x68)]
  MinimapSignal,
  #[bin(value = 0x69)]
  DialogButtonClick,
  #[bin(value = 0x6A)]
  DialogClick,
  #[bin(value = 0x6B)]
  SyncStoreInteger,
  #[bin(value = 0x6C)]
  SyncStoreReal,
  #[bin(value = 0x6D)]
  SyncStoreBoolean,
  #[bin(value = 0x6E)]
  SyncStoreUnit,
  #[bin(value = 0x70)]
  SyncClearInteger,
  #[bin(value = 0x71)]
  SyncClearReal,
  #[bin(value = 0x72)]
  SyncClearBoolean,
  #[bin(value = 0x73)]
  SyncClearUnit,
  #[bin(value = 0x75)]
  ArrowKey,
  #[bin(value = 0x76)]
  Mouse,
  // #[bin(value = 0x77)]
  // W3API,
  #[bin(value = 0x77)]
  SyncData,
  #[bin(value = 0x78)]
  FrameEvent,
  #[bin(value = 0x79)]
  KeyEvent,
  #[bin(value = 0x7A)]
  CommandClick,


  // #[bin(value = 0x94)]
  // Unknown0x94,
  // #[bin(value = 0x74)]
  // Unknown0x74,
  UnknownValue(u8),
}

macro_rules! action_enum {
  (
    pub enum Action {
      $(
        $type_id:ident
        $(($data:ty))?
      ),*
    }
  ) => {
    #[derive(Debug)]
    pub enum Action {
      $(
        $type_id
        $(($data))*
        ,
      )*
    }

    impl Action {
      pub fn type_id(&self) -> ActionTypeId {
        match *self {
          $(
            action_enum!(@PATTERN $type_id, $($data),*) =>
            action_enum!(@TYPE_ID $type_id, $($data),*)
          ),*
        }
      }
    }

    impl BinDecode for Action {
      const MIN_SIZE: usize = 1;

      const FIXED_SIZE: bool = false;

      fn decode<T: Buf>(buf: &mut T) -> Result<Self, BinDecodeError> {
        buf.check_size(1)?;

        // Read the first byte to get the action type id
        let mut byte = buf.get_u8();
        if (byte > 0x77) {
          byte = byte - 1;
        }
        let buf2 = [byte];
        let mut buff = &buf2[..];
        let type_id = ActionTypeId::decode(&mut buff)?;
        match type_id {
          $(
            action_enum!(@TYPE_ID $type_id, $($data),*) =>
            action_enum!(@DECODE buf, $type_id, $($data),*),
          )*
          ActionTypeId::UnknownValue(v) => Err(BinDecodeError::failure(format!("unknown action type id: 0x{:X}", v)))
        }
      }
    }
  };

  (@TYPE_ID $type_id:ident, $data:ty) => {
    ActionTypeId::$type_id
  };

  (@TYPE_ID $type_id:ident,) => {
    ActionTypeId::$type_id
  };

  (@PATTERN $type_id:ident, $data:ty) => {
    Self::$type_id(_)
  };

  (@PATTERN $type_id:ident,) => {
    Self::$type_id
  };

  (@DECODE $buf:expr, $type_id:ident, $data:ty) => {
    Ok(Self::$type_id(<$data>::decode($buf)?))
  };

  (@DECODE $buf:expr, $type_id:ident,) => {
    Ok(Self::$type_id)
  };
}

action_enum! {
  pub enum Action {
    PauseGame(PauseGame),
    ResumeGame,
    SetGameSpeed(SetGameSpeed),
    IncGameSpeed,
    DecGameSpeed,
    SaveGame(SaveGame),
    SaveGameFinished(SaveGameFinished),
    UnitOrderBasic(UnitOrderBasic),
    UnitOrderTargetPoint(UnitOrderTargetPoint),
    UnitOrderTargetImage(UnitOrderTargetImage),
    UnitOrderTargetImage2(UnitOrderTargetImage2),
    UnitOrderTargetImageFogged(UnitOrderTargetImageFogged),
    UnitOrderTargetImageFogged2(UnitOrderTargetImageFogged2),
    ChangeSelection(ChangeSelection),
    AssignGroupHotkey(AssignGroupHotkey),
    SelectGroupHotkey(SelectGroupHotkey),
    SelectSubgroup(SelectSubgroup),
    RefreshSubgroup,
    UnitSelectionEvent(UnitSelectionEvent),
    SelectableSelectionModify(SelectableSelectionModify),
    CancelHeroRevival(CancelHeroRevival),
    RemoveUnitFromBuildingQueue(RemoveUnitFromBuildingQueue),
    ChangeAllyOptions(ChangeAllyOptions),
    TransferResources(TransferResources),
    MapTriggerChatCommand(MapTriggerChatCommand),
    EscPressed,
    ResumeTriggerExec(ResumeTriggerExec),
    TriggerSyncReady(TriggerSyncReady),
    TrackableHit(TrackableHit),
    TrackableTrack(TrackableTrack),
    EnterChooseHeroSkillSubmenu,
    EnterChooseBuildingSubmenu,
    MinimapSignal(MinimapSignal),
    DialogButtonClick(DialogButtonClick),
    DialogClick(DialogClick),
    SyncStoreInteger(SyncStoreInteger),
    SyncStoreReal(SyncStoreReal),
    SyncStoreBoolean(SyncStoreBoolean),
    SyncStoreUnit(SyncStoreUnit),
    SyncClearInteger(SyncCache),
    SyncClearReal(SyncCache),
    SyncClearBoolean(SyncCache),
    SyncClearUnit(SyncCache),
    ArrowKey(ArrowKey),
    Mouse(Mouse),
    //W3API(W3API),
    SyncData(SyncData),
    FrameEvent(FrameEvent),
    KeyEvent(KeyEvent),
    CommandClick(CommandClick)

    // Unknown0x94(Unknown<4>),
    // Unknown0x74(Unknown<2>),
  }
}

#[derive(Debug, BinDecode)]
pub struct PauseGame {
  pub no_commands: u8,
}

#[derive(Debug, BinDecode)]
pub struct SetGameSpeed {
  pub speed: u8,
}

#[derive(Debug, BinDecode)]
pub struct SaveGame {
  pub name: CString,
  pub filename: CString,
  pub quicksave: u8,
}

#[derive(Debug, BinDecode)]
pub struct SaveGameFinished {
  pub success: u32,
}

#[derive(Debug, BinDecode)]
pub struct NetTag {
  pub tag1: u32,
  pub tag2: u32,
}

#[derive(Debug, BinDecode)]
pub struct Vec2 {
  pub x: f32,
  pub y: f32,
}

#[derive(Debug, BinDecode)]
pub struct UnitOrderBasic {
  pub flags: u16,
  pub order: u32,
  pub handle: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct UnitOrderTargetPoint {
  pub base: UnitOrderBasic,
  pub target: Vec2,
}

#[derive(Debug, BinDecode)]
pub struct UnitOrderTargetImage {
  pub base: UnitOrderTargetPoint,
  pub target_object: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct UnitOrderTargetImage2 {
  pub base: UnitOrderTargetImage,
  pub object2: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct UnitOrderTargetImageFogged {
  pub base: UnitOrderTargetPoint,
  pub ghost_image_id: u32,
  pub ghost_flags: u32,
  pub ghost_category: u32,
  pub ghost_owner: u8,
  pub ghost_position: Vec2,
}

#[derive(Debug, BinDecode)]
pub struct UnitOrderTargetImageFogged2 {
  pub base: UnitOrderTargetImageFogged,
  pub target_object: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct ChangeSelection {
  pub select_mode: u8,
  pub units_buildings_number: u16,
  #[bin(repeat = "units_buildings_number")]
  pub selected_objects: Vec<NetTag>,
}

#[derive(Debug, BinDecode)]
pub struct AssignGroupHotkey {
  pub group_number: u8,
  pub selected_object_number: u16,
  #[bin(repeat = "selected_object_number")]
  pub selected_objects: Vec<NetTag>,
}

#[derive(Debug, BinDecode)]
pub struct SelectGroupHotkey {
  pub group_number: u8,
  pub op: u8,
}

#[derive(Debug, BinDecode)]
pub struct SelectSubgroup {
  pub item_id: u32,
  pub object: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct UnitSelectionEvent {
  pub op: u8,
  pub object: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct SelectableSelectionModify {
  pub op: u8,
  pub object: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct CancelHeroRevival {
  pub object: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct RemoveUnitFromBuildingQueue {
  pub slot_number: u8,
  pub item_id: u32,
}

#[derive(Debug, BinDecode)]
pub struct ChangeAllyOptions {
  pub player_slot_number: u8,
  pub flags: u32,
}

#[derive(Debug, BinDecode)]
pub struct TransferResources {
  pub player_slot_number: u8,
  pub gold_to_transfer: u32,
  pub lumber_to_transfer: u32,
}

#[derive(Debug, BinDecode)]
pub struct MapTriggerChatCommand {
  pub trigger: NetTag,
  pub chat_command: CString,
}

#[derive(Debug, BinDecode)]
pub struct ResumeTriggerExec {
  pub trigger: NetTag,
  pub sleep_id: u32,
}

#[derive(Debug, BinDecode)]
pub struct TriggerSyncReady {
  pub trigger: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct TrackableHit {
  pub trackable: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct TrackableTrack {
  pub trackable: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct MinimapSignal {
  pub location: Vec2,
  pub duration: f32,
}

#[derive(Debug, BinDecode)]
pub struct DialogButtonClick {
  _unknown_a: NetTag,
  _unknown_b: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct DialogClick {
  _unknown_a: NetTag,
  _unknown_b: NetTag,
}

#[derive(Debug, BinDecode)]
pub struct SyncCache {
  pub campaign_key: CString,
  pub mission_key: CString,
  pub key: CString,
}

#[derive(Debug, BinDecode)]
pub struct SyncStoreInteger {
  pub cache: SyncCache,
  pub val: u32,
}

#[derive(Debug, BinDecode)]
pub struct SyncStoreReal {
  pub cache: SyncCache,
  pub val: f32,
}

#[derive(Debug, BinDecode)]
pub struct SyncStoreBoolean {
  pub cache: SyncCache,
  pub val: u32,
}

#[derive(Debug, BinDecode)]
pub struct SyncAbility {
  pub ability_id: u32,
  pub level: u32,
}

#[derive(Debug, BinDecode)]
pub struct SyncItem {
  pub item_id: u32,
  pub charges: u32,
  pub flags: u32,
}

#[derive(Debug, BinDecode)]
pub struct SyncHeroData {
  pub xp: u32,
  pub level: u32,
  pub skill_points: u32,
  pub proper_name_id: u32,
  pub str: u32,
  pub str_bonus: f32,
  pub agi: u32,
  pub speed_mod: f32,
  pub cooldown_mod: f32,
  pub agi_bonus: f32,
  pub intel: u32,
  pub int_bonus: f32,
  pub hero_abil_count: u32,
  #[bin(repeat = "hero_abil_count")]
  pub hero_abils: Vec<SyncAbility>,
  pub max_life: f32,
  pub max_mana: f32,
  // >= 6030
  pub sight: f32,
  pub damage_count: u32,
  #[bin(repeat = "damage_count")]
  pub damage: Vec<u32>,
  pub defense: f32,
  // >= 6031
  pub control_groups: u16,
}

#[derive(Debug, BinDecode)]
pub struct SyncUnit {
  pub unit_id: u32,
  pub item_count: u32,
  #[bin(repeat = "item_count")]
  pub items: Vec<SyncItem>,
  pub hero_data: SyncHeroData,
}

#[derive(Debug, BinDecode)]
pub struct SyncStoreUnit {
  pub cache: SyncCache,
  pub val: SyncUnit,
}

#[derive(Debug, BinDecode)]
pub struct ArrowKey {
  pub event: u8,
}

#[derive(Debug, BinDecode)]
pub struct Mouse {
  pub event: u8,
  pub position: Vec2,
  pub button: u8,
}

// #[derive(Debug, BinDecode)]
// pub struct W3API {
//   pub command: u32,
//   pub data: u32,
//   pub buffer_length: u32,
//   #[bin(repeat = "buffer_length")]
//   pub buffer: Vec<u8>,
// }

#[derive(Debug, BinDecode)]
pub struct SyncData {
  pub prefix: CString,
  pub data: CString,
  pub from_server: u32,
}

#[derive(Debug, BinDecode)]
pub struct FrameEvent {
  pub frame: NetTag,
  pub event: u32,
  pub event_data: f32,
  pub event_data2: CString,
}

#[derive(Debug, BinDecode)]
pub struct KeyEvent {
  pub unknown: NetTag,
  pub event: u32,
  pub key: u32,
  pub meta_key: u32,
}

#[derive(Debug, BinDecode)]
pub struct CommandClick {
  pub unknown: NetTag,
  pub ability_id: u32,
  pub order_id: u32,
}

#[derive(Debug)]
pub struct Unknown<const SIZE: usize> {
  _unknown: [u8; SIZE],
}

impl<const SIZE: usize> BinDecode for Unknown<SIZE> {
  const MIN_SIZE: usize = SIZE;
  const FIXED_SIZE: bool = true;
  fn decode<T: Buf>(buf: &mut T) -> Result<Self, BinDecodeError> {
    buf.check_size(SIZE)?;
    let mut data = [0_u8; SIZE];
    buf.copy_to_slice(&mut data);
    Ok(Self { _unknown: data })
  }
}
