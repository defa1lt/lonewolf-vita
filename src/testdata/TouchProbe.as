package {
    import flash.display.Sprite;
    import flash.events.MouseEvent;
    import flash.events.TouchEvent;
    import flash.ui.Multitouch;
    import flash.ui.MultitouchInputMode;
    public class TouchProbe extends Sprite {
        public var begins:int = 0;
        public var ends:int = 0;
        public var moves:int = 0;
        public var taps:int = 0;
        public var outs:int = 0;
        public var clicks:int = 0;
        public var active:int = 0;
        public var peak:int = 0;
        public var stagePointX:Number;
        public var stagePointY:Number;
        public var localPointX:Number;
        public var localPointY:Number;
        public var targetName:String;
        public var id:int;
        public var primary:Boolean;
        public var bubbleTarget:String;
        public function TouchProbe() {
            Multitouch.inputMode = MultitouchInputMode.TOUCH_POINT;
            var parent:Sprite = new Sprite();
            parent.x = 50; parent.y = 30;
            parent.scaleX = 2; parent.scaleY = 2;
            addChild(parent);
            var child:Sprite = new Sprite();
            child.name = "zoomIn";
            child.graphics.beginFill(0xFFFFFF);
            child.graphics.drawRect(10, 20, 60, 50);
            parent.addChild(child);
            child.addEventListener(TouchEvent.TOUCH_BEGIN, function(e:TouchEvent):void {
                begins++; active++; peak = Math.max(peak, active);
                stagePointX=e.stageX; stagePointY=e.stageY;
                localPointX=e.localX; localPointY=e.localY;
                targetName=e.target.name; id=e.touchPointID; primary=e.isPrimaryTouchPoint;
            });
            parent.addEventListener(TouchEvent.TOUCH_BEGIN, function(e:TouchEvent):void { bubbleTarget=e.target.name; });
            child.addEventListener(TouchEvent.TOUCH_MOVE, function(e:TouchEvent):void { moves++; });
            stage.addEventListener(TouchEvent.TOUCH_END, function(e:TouchEvent):void { ends++; active--; });
            child.addEventListener(TouchEvent.TOUCH_TAP, function(e:TouchEvent):void { taps++; });
            child.addEventListener(TouchEvent.TOUCH_OUT, function(e:TouchEvent):void { outs++; });
            child.addEventListener(MouseEvent.CLICK, function(e:MouseEvent):void { clicks++; });
        }
    }
}
