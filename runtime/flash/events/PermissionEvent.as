package flash.events {
    public final class PermissionEvent extends Event {
        public static const PERMISSION_STATUS:String = "permissionStatus";

        private var _status:String;

        public function PermissionEvent(type:String, bubbles:Boolean = false,
                                        cancelable:Boolean = false, status:String = "denied") {
            super(type, bubbles, cancelable);
            _status = status;
        }

        public function get status():String {
            return _status;
        }

        override public function clone():Event {
            return new PermissionEvent(type, bubbles, cancelable, _status);
        }

        override public function toString():String {
            return formatToString("PermissionEvent", "type", "bubbles", "cancelable", "eventPhase", "status");
        }
    }
}
