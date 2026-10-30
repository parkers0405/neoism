//! Native host paint data over the canonical catalog/filter/selection engines.
use super::{
    commands::{CommandService, COMMANDS},
    modes::PaletteRow,
    CommandPalette, *,
};
use serde_json::{json, Value};

pub fn host_command_catalog() -> Value {
    Value::Array(COMMANDS.iter().map(|c|json!({"title":c.title,"shortcut":c.shortcut,"action":format!("{:?}",c.action),"service":c.service.prefix(),"glyph":c.service.icon_themed()})).collect())
}
impl CommandPalette {
    pub fn host_set_management_enabled(&mut self, enabled: bool) { self.host_management_enabled = enabled; }
    pub fn host_set_supported_actions(&mut self, actions: Option<Vec<String>>) { self.host_supported_actions=actions; self.selected_index=0; }
    pub fn host_frame(&mut self, width: f32, height: f32) -> Value {
        self.viewport_height = height;
        let count = self.filtered_rows().len();
        self.clamp_scroll(count);
        let base = self.active_rect(width, 1.0).unwrap_or([0.0; 4]);
        let mut frame = self.host_motion_frame(base);
        let rows:Vec<Value>=self.filtered_rows().iter().enumerate().map(|(index,(score,row))|{
            let (service,action,glyph)=match row {
                PaletteRow::Command{service,action,..}=>(*service,Some(format!("{action:?}")),CommandService::from_prefix(service).map(|s|s.icon_themed()).unwrap_or("\u{f0c9}")),
                _=>("neoism",row.action().map(|a|format!("{a:?}")),"\u{f0c9}"),
            };
            json!({"index":index,"score":score,"title":row.title(),"displayTitle":row.display_title(),"shortcut":row.shortcut(),"action":action,"service":service,"glyph":glyph,"exCommand":match row{PaletteRow::Ex{name,..}=>Some(*name),_=>None}})
        }).collect();
        frame["rows"] = json!(rows);
        frame["target"] = self.get_selected_action().map(|a|json!({"action":format!("{a:?}")}))
            .or_else(||self.get_selected_ex_command().map(|command|json!({"action":"ExCommand","argument":command})))
            .or_else(||self.get_typed_cd_target().map(|path|json!({"action":"ChangeWorkspaceDirectory","argument":path}))).unwrap_or(Value::Null);
        frame
    }
    /// Sample only the springs. The host retains the unanimated card bounds
    /// until input/layout changes; motion must not refilter the command catalog.
    pub fn host_motion_frame(&mut self, base: [f32;4]) -> Value {
        let offset = self.tick_list_scroll();
        let cursor = self.tick_cursor();
        let (pop_scale,pop_offset_y) = self.open_pop_transform(self.scale);
        let rect = if self.enabled { [base[0]+base[2]*(1.0-pop_scale)*0.5,base[1]+pop_offset_y,base[2]*pop_scale,base[3]*pop_scale] } else { [0.0;4] };
        let stroke = (crate::panels::file_tree::FRAME_STROKE * self.scale).max(2.0);
        let input_h = self.input_band_height.max(INPUT_HEIGHT * self.scale);
        let inner_w = (rect[2] - stroke * 2.0).max(1.0);
        let row_h = self.row_height();
        let top = stroke + input_h + SEPARATOR_HEIGHT;
        json!({"rect":rect,"query":self.query,"inputRect":[stroke,stroke,inner_w,input_h],"listRect":[stroke,top,inner_w,(rect[3]-top-stroke).max(0.0)],
            "selected":self.selected_index,"selectedY":(self.selected_index as i64-self.scroll_offset as i64)as f32*row_h+offset+cursor,"top":self.scroll_offset,"offset":offset,
            "rowHeight":row_h,"fontSize":RESULT_FONT_SIZE*self.scale,"inputFontSize":INPUT_FONT_SIZE*self.scale,"shortcutFontSize":SHORTCUT_FONT_SIZE*self.scale,"stroke":stroke,"radius":PALETTE_CORNER_RADIUS*self.scale,
            "animating":self.is_animating(),"visibleRows":self.viewport_row_limit(),"cdQuery":self.is_cd_query()})
    }
}
